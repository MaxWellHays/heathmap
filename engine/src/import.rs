//! Importing your runs: GPS tracks from a Strava export (the whole ZIP from "Download your
//! data", or single GPX / TCX / FIT files, gzipped or not) or from the older
//! `strava_runs.json`. Files arrive by drag and drop or the panel's Import button.
//!
//! Runs are personal data, so they never go into the repo or the level: imported tracks are
//! kept on this machine only (native: `~/.local/share/heathmap/runs.bin` or the platform's
//! equivalent; browser: IndexedDB) as WGS84 coordinates, and placed in the level with its
//! georeference when loaded. Only runs that pass through the level are kept.

use std::collections::HashMap;
use std::io::Read;
use std::sync::Mutex;

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, poll_once};

use crate::level::{Level, LevelState};
use crate::runs::{Run, Runs};

/// One GPS point: degrees, and seconds since the run started.
#[derive(Clone, Copy, Debug)]
pub struct TrackPoint {
    pub lat: f64,
    pub lon: f64,
    pub t: f32,
}

/// A recorded run as imported (not yet placed in a level).
#[derive(Clone, Debug)]
pub struct Track {
    /// Strava activity id where known, otherwise derived from the start time.
    pub id: u64,
    /// Start time, Unix seconds (UTC).
    pub start: i64,
    pub points: Vec<TrackPoint>,
}

#[derive(PartialEq)]
enum Sport {
    Run,
    Other,
    Unknown,
}

/// What came out of a batch of files.
#[derive(Default)]
struct ImportReport {
    tracks: Vec<Track>,
    not_runs: usize,
    failed: Vec<String>,
}

// ---------------------------------------------------------------------------------------
// Parsing

fn parse_files(files: Vec<(String, Vec<u8>)>) -> ImportReport {
    let mut report = ImportReport::default();
    for (name, bytes) in files {
        parse_file(&name, bytes, None, &mut report);
    }
    report
}

/// Parses one file into `report`. `known` carries what a Strava export's activities.csv
/// says about it: (activity id, is a run).
fn parse_file(name: &str, bytes: Vec<u8>, known: Option<(u64, bool)>, report: &mut ImportReport) {
    let lower = name.to_lowercase();
    if let Some(inner) = lower.strip_suffix(".gz") {
        let mut out = Vec::new();
        match flate2::read::MultiGzDecoder::new(&bytes[..]).read_to_end(&mut out) {
            Ok(_) => parse_file(inner, out, known, report),
            Err(e) => report.failed.push(format!("{name}: {e}")),
        }
        return;
    }
    if known.is_some_and(|(_, run)| !run) {
        report.not_runs += 1;
        return;
    }
    let ext = lower.rsplit('.').next().unwrap_or("");
    let parsed = match ext {
        "zip" => return parse_zip(name, &bytes, report),
        "json" => return parse_strava_json(name, &bytes, report),
        "gpx" => parse_xml(&bytes, Xml::Gpx),
        "tcx" => parse_xml(&bytes, Xml::Tcx),
        "fit" => parse_fit(&bytes),
        _ => return, // other files in a folder or export (photos, CSVs…)
    };
    match parsed {
        Ok((sport, start, points)) => {
            if sport == Sport::Other {
                report.not_runs += 1;
            } else if points.len() >= 2 {
                let id = known.map(|k| k.0).or_else(|| id_from_name(name)).unwrap_or(start as u64);
                report.tracks.push(Track { id, start, points });
            }
        }
        Err(e) => report.failed.push(format!("{name}: {e}")),
    }
}

/// Strava names exported activity files by their id: `activities/1234567890.fit.gz`.
fn id_from_name(name: &str) -> Option<u64> {
    let file = name.rsplit(['/', '\\']).next()?;
    file.split('.').next()?.parse().ok()
}

/// A Strava export: activities.csv tells which files are runs; activities/ holds the tracks.
fn parse_zip(name: &str, bytes: &[u8], report: &mut ImportReport) {
    let mut zip = match zip::ZipArchive::new(std::io::Cursor::new(bytes)) {
        Ok(z) => z,
        Err(e) => return report.failed.push(format!("{name}: {e}")),
    };
    let mut known: HashMap<String, (u64, bool)> = HashMap::new();
    if let Ok(mut csv_file) = zip.by_name("activities.csv") {
        let mut text = Vec::new();
        if csv_file.read_to_end(&mut text).is_ok() {
            known = activity_types(&text);
        }
    }
    for i in 0..zip.len() {
        let Ok(mut entry) = zip.by_index(i) else { continue };
        let entry_name = entry.name().to_string();
        let lower = entry_name.to_lowercase();
        let track_file = [".gpx", ".tcx", ".fit"].iter().any(|e| lower.ends_with(e) || lower.ends_with(&format!("{e}.gz")));
        if !entry.is_file() || !track_file {
            continue;
        }
        let k = known.get(&entry_name).copied();
        if k.is_some_and(|(_, run)| !run) {
            report.not_runs += 1;
            continue;
        }
        let mut data = Vec::with_capacity(entry.size() as usize);
        if let Err(e) = entry.read_to_end(&mut data) {
            report.failed.push(format!("{entry_name}: {e}"));
            continue;
        }
        parse_file(&entry_name, data, k, report);
    }
}

/// From activities.csv: file name → (activity id, whether it's a run).
fn activity_types(csv_bytes: &[u8]) -> HashMap<String, (u64, bool)> {
    let mut out = HashMap::new();
    let mut reader = csv::ReaderBuilder::new().flexible(true).from_reader(csv_bytes);
    let Ok(headers) = reader.headers().cloned() else { return out };
    let column = |name: &str| headers.iter().position(|h| h.eq_ignore_ascii_case(name));
    let (Some(id_col), Some(type_col), Some(file_col)) = (column("Activity ID"), column("Activity Type"), column("Filename")) else {
        return out;
    };
    for record in reader.records().flatten() {
        let (Some(id), Some(kind), Some(file)) = (record.get(id_col), record.get(type_col), record.get(file_col)) else { continue };
        if let (Ok(id), false) = (id.parse::<u64>(), file.is_empty()) {
            // "Run", "Trail Run", "Virtual Run"… (virtual runs have no GPS and drop out later).
            out.insert(file.to_string(), (id, kind.to_lowercase().contains("run")));
        }
    }
    out
}

/// The older `strava_runs.json` (from the Strava API): {"runs": [{id, start_epoch, latlng, time}]}.
fn parse_strava_json(name: &str, bytes: &[u8], report: &mut ImportReport) {
    #[derive(serde::Deserialize)]
    struct File {
        runs: Vec<JsonRun>,
    }
    #[derive(serde::Deserialize)]
    struct JsonRun {
        id: u64,
        #[serde(default)]
        start_epoch: i64,
        #[serde(default)]
        latlng: Vec<[f64; 2]>,
        #[serde(default)]
        time: Vec<f32>,
    }
    match serde_json::from_slice::<File>(bytes) {
        Ok(file) => {
            for r in file.runs {
                if r.latlng.len() >= 2 && r.latlng.len() == r.time.len() {
                    let points = r.latlng.iter().zip(&r.time).map(|(ll, &t)| TrackPoint { lat: ll[0], lon: ll[1], t }).collect();
                    report.tracks.push(Track { id: r.id, start: r.start_epoch, points });
                }
            }
        }
        Err(e) => report.failed.push(format!("{name}: {e}")),
    }
}

type Parsed = Result<(Sport, i64, Vec<TrackPoint>), String>;

#[derive(Clone, Copy, PartialEq)]
enum Xml {
    Gpx,
    Tcx,
}

/// GPX (`<trkpt lat lon><time>`) and TCX (`<Trackpoint><Time><Position><LatitudeDegrees>…`).
fn parse_xml(bytes: &[u8], format: Xml) -> Parsed {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(bytes);
    let mut buf = Vec::new();
    let mut sport = Sport::Unknown;
    let mut raw: Vec<(f64, f64, i64)> = Vec::new();
    // Current point being read, and which element's text comes next.
    let (mut lat, mut lon, mut time): (Option<f64>, Option<f64>, Option<i64>) = (None, None, None);
    let mut text_of: Vec<u8> = Vec::new();
    let mut in_point = false;
    loop {
        let event = reader.read_event_into(&mut buf).map_err(|e| e.to_string())?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let tag = e.local_name().as_ref().to_vec();
                match (format, tag.as_slice()) {
                    (Xml::Gpx, b"trkpt") => {
                        in_point = true;
                        let attr = |k: &str| {
                            e.try_get_attribute(k).ok().flatten().and_then(|a| std::str::from_utf8(&a.value).ok()?.trim().parse::<f64>().ok())
                        };
                        (lat, lon, time) = (attr("lat"), attr("lon"), None);
                    }
                    (Xml::Tcx, b"Trackpoint") => {
                        in_point = true;
                        (lat, lon, time) = (None, None, None);
                    }
                    (Xml::Tcx, b"Activity") => {
                        if let Ok(Some(a)) = e.try_get_attribute("Sport") {
                            let s = String::from_utf8_lossy(&a.value).to_lowercase();
                            sport = if s.contains("run") { Sport::Run } else if s == "other" { Sport::Unknown } else { Sport::Other };
                        }
                    }
                    _ => {}
                }
                if matches!(event, Event::Start(_)) {
                    text_of = tag;
                }
            }
            Event::Text(ref t) => {
                let text = t.decode().map_err(|e| e.to_string())?;
                let text = text.trim();
                match (format, text_of.as_slice()) {
                    (_, b"time" | b"Time") if in_point => time = parse_iso_time(text),
                    (Xml::Tcx, b"LatitudeDegrees") if in_point => lat = text.parse().ok(),
                    (Xml::Tcx, b"LongitudeDegrees") if in_point => lon = text.parse().ok(),
                    // Strava writes <type>running</type> (or a number: 9 is running).
                    (Xml::Gpx, b"type") if !in_point => {
                        let s = text.to_lowercase();
                        sport = if s.contains("run") || s == "9" { Sport::Run } else { Sport::Other };
                    }
                    _ => {}
                }
            }
            Event::End(ref e) => {
                let tag = e.local_name();
                if (format == Xml::Gpx && tag.as_ref() == b"trkpt") || (format == Xml::Tcx && tag.as_ref() == b"Trackpoint") {
                    if let (Some(a), Some(o), Some(t)) = (lat, lon, time) {
                        raw.push((a, o, t));
                    }
                    in_point = false;
                }
                text_of.clear();
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    finish_points(sport, raw)
}

/// FIT: `record` messages carry position (semicircles) and timestamp; `session` the sport.
fn parse_fit(bytes: &[u8]) -> Parsed {
    use fitparser::{Value, profile::MesgNum};
    let records = fitparser::from_bytes(bytes).map_err(|e| e.to_string())?;
    let mut sport = Sport::Unknown;
    let mut raw = Vec::new();
    const SEMICIRCLE: f64 = 180.0 / 2_147_483_648.0;
    for record in &records {
        match record.kind() {
            MesgNum::Record => {
                let (mut lat, mut lon, mut time) = (None, None, None);
                for f in record.fields() {
                    match (f.name(), f.value()) {
                        ("position_lat", Value::SInt32(v)) => lat = Some(*v as f64 * SEMICIRCLE),
                        ("position_long", Value::SInt32(v)) => lon = Some(*v as f64 * SEMICIRCLE),
                        ("timestamp", Value::Timestamp(t)) => time = Some(t.timestamp()),
                        _ => {}
                    }
                }
                if let (Some(a), Some(o), Some(t)) = (lat, lon, time) {
                    raw.push((a, o, t));
                }
            }
            MesgNum::Session | MesgNum::Sport => {
                for f in record.fields() {
                    if let ("sport", Value::String(s)) = (f.name(), f.value()) {
                        sport = if s.contains("running") { Sport::Run } else { Sport::Other };
                    }
                }
            }
            _ => {}
        }
    }
    finish_points(sport, raw)
}

/// Absolute (lat, lon, Unix time) points to a track: time relative to the first point.
fn finish_points(sport: Sport, raw: Vec<(f64, f64, i64)>) -> Parsed {
    let start = raw.first().map_or(0, |p| p.2);
    let points = raw
        .into_iter()
        .filter(|p| p.0.abs() <= 90.0 && p.1.abs() <= 180.0 && !(p.0 == 0.0 && p.1 == 0.0))
        .map(|(lat, lon, t)| TrackPoint { lat, lon, t: (t - start) as f32 })
        .collect();
    Ok((sport, start, points))
}

/// "2026-09-27T11:56:40Z", with optional fractional seconds and "+01:00"-style offsets, to Unix seconds.
fn parse_iso_time(s: &str) -> Option<i64> {
    let num = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, sec) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let rest = &s[19..];
    let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match rest.as_bytes().first() {
        Some(sign @ (b'+' | b'-')) => {
            let digits: String = rest[1..].chars().filter(char::is_ascii_digit).collect();
            let (oh, om) = (digits.get(0..2)?.parse::<i64>().ok()?, digits.get(2..4).and_then(|m| m.parse().ok()).unwrap_or(0));
            let o = oh * 3600 + om * 60;
            if *sign == b'+' { o } else { -o }
        }
        _ => 0,
    };
    // Days from civil (Howard Hinnant's algorithm).
    let (y2, m2) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + sec - offset)
}

// ---------------------------------------------------------------------------------------
// Stored runs: b"HMR1", u32 count, then per run: u64 id, i64 start, u32 n,
// n × (i32 lat × 1e7, i32 lon × 1e7, f32 seconds), all little-endian.

const MAGIC: &[u8; 4] = b"HMR1";

fn encode(tracks: &[Track]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend((tracks.len() as u32).to_le_bytes());
    for t in tracks {
        out.extend(t.id.to_le_bytes());
        out.extend(t.start.to_le_bytes());
        out.extend((t.points.len() as u32).to_le_bytes());
        for p in &t.points {
            out.extend(((p.lat * 1e7).round() as i32).to_le_bytes());
            out.extend(((p.lon * 1e7).round() as i32).to_le_bytes());
            out.extend(p.t.to_le_bytes());
        }
    }
    out
}

fn decode(bytes: &[u8]) -> Option<Vec<Track>> {
    let mut pos = 0;
    let mut take = |n: usize| -> Option<&[u8]> {
        let s = bytes.get(pos..pos + n)?;
        pos += n;
        Some(s)
    };
    if take(4)? != MAGIC {
        return None;
    }
    let u32_ = |b: &[u8]| u32::from_le_bytes(b.try_into().unwrap());
    let count = u32_(take(4)?);
    let mut tracks = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let id = u64::from_le_bytes(take(8)?.try_into().ok()?);
        let start = i64::from_le_bytes(take(8)?.try_into().ok()?);
        let n = u32_(take(4)?) as usize;
        let mut points = Vec::with_capacity(n);
        for _ in 0..n {
            let b = take(12)?;
            let lat = i32::from_le_bytes(b[0..4].try_into().ok()?) as f64 / 1e7;
            let lon = i32::from_le_bytes(b[4..8].try_into().ok()?) as f64 / 1e7;
            points.push(TrackPoint { lat, lon, t: f32::from_le_bytes(b[8..12].try_into().ok()?) });
        }
        tracks.push(Track { id, start, points });
    }
    Some(tracks)
}

// ---------------------------------------------------------------------------------------
// Plugin

/// Things arriving from outside the frame loop (file dialogs, browser callbacks).
enum Delivery {
    /// Files to import: (name, contents).
    Files(Vec<(String, Vec<u8>)>),
    /// The runs saved earlier on this machine.
    Stored(Vec<u8>),
}

static INBOX: Mutex<Vec<Delivery>> = Mutex::new(Vec::new());

fn deliver(d: Delivery) {
    INBOX.lock().unwrap_or_else(|e| e.into_inner()).push(d);
}

/// All your imported runs (WGS84), as stored on this machine.
#[derive(Resource, Default)]
pub struct RunStore(pub Vec<Track>);

/// Import progress and the last result, for the panel.
#[derive(Resource, Default)]
pub struct ImportStatus {
    pub busy: bool,
    pub message: Option<String>,
}

/// Set from the panel.
#[derive(Resource, Default)]
pub struct ImportRequest {
    /// Open the file picker.
    pub pick_files: bool,
    /// Delete all imported runs from this machine.
    pub forget: bool,
}

#[derive(Resource, Default)]
struct ImportTasks(Vec<Task<ImportReport>>);

pub struct ImportPlugin;

impl Plugin for ImportPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RunStore>()
            .init_resource::<ImportStatus>()
            .init_resource::<ImportRequest>()
            .init_resource::<ImportTasks>()
            .add_systems(Startup, platform::start)
            .add_systems(Update, (platform::dropped_files, receive, handle_requests))
            .add_systems(Update, (finish_imports, place_runs).chain().run_if(in_state(LevelState::Ready)));
    }
}

/// Takes deliveries: stored runs go straight into the store, files are parsed in the background.
fn receive(mut store: ResMut<RunStore>, mut tasks: ResMut<ImportTasks>, mut status: ResMut<ImportStatus>) {
    let deliveries = std::mem::take(&mut *INBOX.lock().unwrap_or_else(|e| e.into_inner()));
    for d in deliveries {
        match d {
            Delivery::Stored(bytes) => match decode(&bytes) {
                Some(tracks) => {
                    info!("Loaded {} saved runs", tracks.len());
                    merge(&mut store.0, tracks);
                }
                None => warn!("Saved runs are unreadable; import them again"),
            },
            Delivery::Files(files) => {
                status.busy = true;
                status.message = Some(format!("Reading {}…", files.iter().map(|f| f.0.as_str()).collect::<Vec<_>>().join(", ")));
                tasks.0.push(AsyncComputeTaskPool::get().spawn(async move { parse_files(files) }));
            }
        }
    }
}

/// Adds `new` runs not already in `tracks` (by id); returns how many were added.
fn merge(tracks: &mut Vec<Track>, new: Vec<Track>) -> usize {
    let mut added = 0;
    for t in new {
        if !tracks.iter().any(|o| o.id == t.id) {
            tracks.push(t);
            added += 1;
        }
    }
    tracks.sort_by_key(|t| t.start);
    added
}

/// Collects finished imports: keeps the runs that pass through the level and saves them.
fn finish_imports(mut tasks: ResMut<ImportTasks>, mut store: ResMut<RunStore>, mut status: ResMut<ImportStatus>, level: Res<Level>) {
    let mut finished = Vec::new();
    tasks.0.retain_mut(|task| match block_on(poll_once(task)) {
        Some(report) => {
            finished.push(report);
            false
        }
        None => true,
    });
    status.busy = !tasks.0.is_empty();
    for report in finished {
        let Some(georef) = &level.meta.georef else {
            status.message = Some("This level has no georeference: re-run data/export_engine.py".into());
            continue;
        };
        let extent = &level.meta.extent;
        let inside = |p: &TrackPoint| {
            let q = georef.to_level(p.lat, p.lon);
            (extent.x[0]..=extent.x[1]).contains(&q.x) && (extent.z[0]..=extent.z[1]).contains(&q.y)
        };
        let found = report.tracks.len();
        let here: Vec<Track> = report.tracks.into_iter().filter(|t| t.points.iter().any(inside)).collect();
        let in_area = here.len();
        let added = merge(&mut store.0, here);
        if added > 0 {
            platform::save(encode(&store.0));
        }
        let mut msg = format!("Imported {added} new run{}", if added == 1 { "" } else { "s" });
        let mut notes = Vec::new();
        if in_area > added {
            notes.push(format!("{} already there", in_area - added));
        }
        if found > in_area {
            notes.push(format!("{} elsewhere", found - in_area));
        }
        if report.not_runs > 0 {
            notes.push(format!("{} other activities skipped", report.not_runs));
        }
        if !report.failed.is_empty() {
            notes.push(format!("{} unreadable", report.failed.len()));
            for f in &report.failed {
                warn!("Could not read {f}");
            }
        }
        if !notes.is_empty() {
            msg += &format!(" ({})", notes.join(", "));
        }
        info!("{msg}");
        status.message = Some(msg);
    }
}

fn handle_requests(mut request: ResMut<ImportRequest>, mut store: ResMut<RunStore>, mut status: ResMut<ImportStatus>) {
    if std::mem::take(&mut request.pick_files) {
        platform::pick_files();
    }
    if std::mem::take(&mut request.forget) {
        store.0.clear();
        platform::delete();
        status.message = Some("Your runs were removed from this device".into());
    }
}

/// Places the stored runs in the level whenever they change.
fn place_runs(store: Res<RunStore>, level: Res<Level>, mut runs: ResMut<Runs>, mut placed: Local<bool>) {
    if !store.is_changed() && *placed {
        return;
    }
    *placed = true;
    let Some(georef) = &level.meta.georef else {
        if !store.0.is_empty() {
            warn!("This level has no georeference: re-run data/export_engine.py to show your runs");
        }
        return;
    };
    runs.0 = store
        .0
        .iter()
        .map(|t| Run::new(t.id, t.start, t.points.iter().map(|p| (georef.to_level(p.lat, p.lon), p.t)).collect()))
        .filter(|r| r.points.len() >= 2)
        .collect();
}

// ---------------------------------------------------------------------------------------
// Native: a file in the user's data directory, a file dialog, dropped files and paths
// given on the command line.

#[cfg(not(target_arch = "wasm32"))]
mod platform {
    use super::*;
    use std::path::{Path, PathBuf};

    fn store_path() -> Option<PathBuf> {
        Some(dirs::data_dir()?.join("heathmap").join("runs.bin"))
    }

    pub fn start() {
        if let Some(bytes) = store_path().and_then(|p| std::fs::read(p).ok()) {
            deliver(Delivery::Stored(bytes));
        }
        // `heathmap-engine export.zip …` imports those files.
        let paths: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).filter(|p| p.exists()).collect();
        if !paths.is_empty() {
            read_and_deliver(paths);
        }
    }

    /// Reads files (and folders, recursively) and hands them to the importer.
    fn read_and_deliver(paths: Vec<PathBuf>) {
        let mut files = Vec::new();
        fn add(path: &Path, files: &mut Vec<(String, Vec<u8>)>) {
            if path.is_dir() {
                for entry in std::fs::read_dir(path).into_iter().flatten().flatten() {
                    add(&entry.path(), files);
                }
            } else if let Ok(bytes) = std::fs::read(path) {
                files.push((path.to_string_lossy().into_owned(), bytes));
            }
        }
        for p in &paths {
            add(p, &mut files);
        }
        if !files.is_empty() {
            deliver(Delivery::Files(files));
        }
    }

    pub fn dropped_files(mut drops: MessageReader<bevy::window::FileDragAndDrop>) {
        let paths: Vec<PathBuf> = drops
            .read()
            .filter_map(|d| match d {
                bevy::window::FileDragAndDrop::DroppedFile { path_buf, .. } => Some(path_buf.clone()),
                _ => None,
            })
            .collect();
        if !paths.is_empty() {
            std::thread::spawn(move || read_and_deliver(paths));
        }
    }

    pub fn pick_files() {
        // The dialog blocks, so it runs on its own thread.
        std::thread::spawn(|| {
            let picked = rfd::FileDialog::new()
                .set_title("Import your runs: a Strava export (.zip) or GPX / TCX / FIT files")
                .add_filter("Strava export or tracks", &["zip", "gpx", "tcx", "fit", "gz", "json"])
                .pick_files();
            if let Some(paths) = picked {
                read_and_deliver(paths);
            }
        });
    }

    pub fn save(bytes: Vec<u8>) {
        let Some(path) = store_path() else { return };
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(path.parent().unwrap())?;
            let tmp = path.with_extension("bin.tmp");
            std::fs::write(&tmp, bytes)?;
            std::fs::rename(tmp, &path)
        };
        match write() {
            Ok(()) => info!("Saved your runs to {}", path.display()),
            Err(e) => warn!("Could not save your runs: {e}"),
        }
    }

    pub fn delete() {
        if let Some(path) = store_path() {
            let _ = std::fs::remove_file(path);
        }
    }
}

// ---------------------------------------------------------------------------------------
// Browser: IndexedDB for storage; files dropped on the page or picked with a file input.

#[cfg(target_arch = "wasm32")]
mod platform {
    use super::*;
    use wasm_bindgen::{JsCast, JsValue, closure::Closure};
    use wasm_bindgen_futures::{JsFuture, spawn_local};
    use web_sys::{DragEvent, Event, FileList, HtmlInputElement, IdbDatabase, IdbOpenDbRequest, IdbRequest, IdbTransactionMode};

    const DB: &str = "heathmap";
    const STORE: &str = "files";
    const KEY: &str = "runs";

    pub fn start() {
        let Some(window) = web_sys::window() else { return };
        // Dropping files anywhere on the page imports them.
        let over = Closure::<dyn FnMut(DragEvent)>::new(|e: DragEvent| e.prevent_default());
        let drop = Closure::<dyn FnMut(DragEvent)>::new(|e: DragEvent| {
            e.prevent_default();
            if let Some(files) = e.data_transfer().and_then(|d| d.files()) {
                read_files(files);
            }
        });
        let _ = window.add_event_listener_with_callback("dragover", over.as_ref().unchecked_ref());
        let _ = window.add_event_listener_with_callback("drop", drop.as_ref().unchecked_ref());
        over.forget();
        drop.forget();

        spawn_local(async {
            match load().await {
                Ok(Some(bytes)) => deliver(Delivery::Stored(bytes)),
                Ok(None) => {}
                Err(e) => warn!("Could not read saved runs: {e:?}"),
            }
        });
    }

    fn read_files(list: FileList) {
        let files: Vec<web_sys::File> = (0..list.length()).filter_map(|i| list.get(i)).collect();
        spawn_local(async move {
            let mut out = Vec::new();
            for f in files {
                match JsFuture::from(f.array_buffer()).await {
                    Ok(buf) => out.push((f.name(), js_sys::Uint8Array::new(&buf).to_vec())),
                    Err(e) => warn!("Could not read {}: {e:?}", f.name()),
                }
            }
            if !out.is_empty() {
                deliver(Delivery::Files(out));
            }
        });
    }

    pub fn dropped_files() {}

    pub fn pick_files() {
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
        let input = match doc.get_element_by_id("run-import").and_then(|e| e.dyn_into::<HtmlInputElement>().ok()) {
            Some(input) => input,
            None => {
                let Ok(input) = doc.create_element("input").map(|e| e.unchecked_into::<HtmlInputElement>()) else { return };
                input.set_id("run-import");
                input.set_type("file");
                input.set_multiple(true);
                input.set_accept(".zip,.gpx,.tcx,.fit,.gz,.json");
                let _ = input.style().set_property("display", "none");
                let changed = Closure::<dyn FnMut(Event)>::new(|e: Event| {
                    if let Some(input) = e.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) {
                        if let Some(files) = input.files() {
                            read_files(files);
                        }
                        input.set_value("");
                    }
                });
                input.set_onchange(Some(changed.as_ref().unchecked_ref()));
                changed.forget();
                if let Some(body) = doc.body() {
                    let _ = body.append_child(&input);
                }
                input
            }
        };
        input.click();
    }

    /// Resolves when an IndexedDB request succeeds (with its result) or fails.
    async fn wait(request: &IdbRequest) -> Result<JsValue, JsValue> {
        let promise = js_sys::Promise::new(&mut |resolve, reject| {
            let r = request.clone();
            let ok = Closure::once_into_js(move || {
                let _ = resolve.call1(&JsValue::NULL, &r.result().unwrap_or(JsValue::UNDEFINED));
            });
            let err = Closure::once_into_js(move || {
                let _ = reject.call1(&JsValue::NULL, &JsValue::from_str("IndexedDB request failed"));
            });
            request.set_onsuccess(Some(ok.unchecked_ref()));
            request.set_onerror(Some(err.unchecked_ref()));
        });
        JsFuture::from(promise).await
    }

    async fn open() -> Result<IdbDatabase, JsValue> {
        let factory = web_sys::window().ok_or("no window")?.indexed_db()?.ok_or("no IndexedDB")?;
        let request: IdbOpenDbRequest = factory.open_with_u32(DB, 1)?;
        let upgrade = Closure::<dyn FnMut(Event)>::new(|e: Event| {
            let db = e
                .target()
                .and_then(|t| t.dyn_into::<IdbOpenDbRequest>().ok())
                .and_then(|r| r.result().ok())
                .and_then(|r| r.dyn_into::<IdbDatabase>().ok());
            if let Some(db) = db {
                let _ = db.create_object_store(STORE);
            }
        });
        request.set_onupgradeneeded(Some(upgrade.as_ref().unchecked_ref()));
        let db = wait(&request).await?;
        drop(upgrade);
        db.dyn_into()
    }

    async fn load() -> Result<Option<Vec<u8>>, JsValue> {
        let db = open().await?;
        let store = db.transaction_with_str(STORE)?.object_store(STORE)?;
        let value = wait(&store.get(&KEY.into())?).await?;
        Ok(value.dyn_into::<js_sys::Uint8Array>().ok().map(|a| a.to_vec()))
    }

    async fn write(bytes: Option<Vec<u8>>) -> Result<(), JsValue> {
        let db = open().await?;
        let store = db.transaction_with_str_and_mode(STORE, IdbTransactionMode::Readwrite)?.object_store(STORE)?;
        let request = match bytes {
            Some(b) => store.put_with_key(&js_sys::Uint8Array::from(&b[..]), &KEY.into())?,
            None => store.delete(&KEY.into())?,
        };
        wait(&request).await.map(|_| ())
    }

    pub fn save(bytes: Vec<u8>) {
        spawn_local(async move {
            if let Err(e) = write(Some(bytes)).await {
                warn!("Could not save your runs: {e:?}");
            }
        });
    }

    pub fn delete() {
        spawn_local(async {
            if let Err(e) = write(None).await {
                warn!("Could not delete your runs: {e:?}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_times() {
        assert_eq!(parse_iso_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso_time("2026-09-27T11:56:40Z"), Some(1_790_510_200));
        assert_eq!(parse_iso_time("2026-09-27T12:56:40.250+01:00"), Some(1_790_510_200));
    }

    #[test]
    fn gpx_track() {
        let gpx = br#"<?xml version="1.0"?><gpx><trk><type>running</type><trkseg>
            <trkpt lat="51.56" lon="-0.16"><ele>90</ele><time>2026-09-27T11:56:40Z</time></trkpt>
            <trkpt lat="51.5601" lon="-0.1601"><time>2026-09-27T11:56:45Z</time></trkpt>
            </trkseg></trk></gpx>"#;
        let (sport, start, points) = parse_xml(gpx, Xml::Gpx).unwrap();
        assert!(sport == Sport::Run);
        assert_eq!(start, 1_790_510_200);
        assert_eq!(points.len(), 2);
        assert_eq!(points[1].t, 5.0);
        assert!((points[1].lon + 0.1601).abs() < 1e-9);
    }

    #[test]
    fn tcx_track() {
        let tcx = br#"<TrainingCenterDatabase><Activities><Activity Sport="Running"><Lap><Track>
            <Trackpoint><Time>2026-09-27T11:56:40Z</Time><Position><LatitudeDegrees>51.56</LatitudeDegrees><LongitudeDegrees>-0.16</LongitudeDegrees></Position></Trackpoint>
            <Trackpoint><Time>2026-09-27T11:56:42Z</Time></Trackpoint>
            <Trackpoint><Time>2026-09-27T11:56:44Z</Time><Position><LatitudeDegrees>51.5602</LatitudeDegrees><LongitudeDegrees>-0.1602</LongitudeDegrees></Position></Trackpoint>
            </Track></Lap></Activity></Activities></TrainingCenterDatabase>"#;
        let (sport, _, points) = parse_xml(tcx, Xml::Tcx).unwrap();
        assert!(sport == Sport::Run);
        assert_eq!(points.len(), 2); // the point without a position is skipped
        assert_eq!(points[1].t, 4.0);
    }

    #[test]
    fn store_round_trip() {
        let tracks = vec![Track { id: 7, start: 1_790_513_800, points: vec![TrackPoint { lat: 51.5612345, lon: -0.1654321, t: 3.5 }] }];
        let back = decode(&encode(&tracks)).unwrap();
        assert_eq!(back[0].id, 7);
        assert!((back[0].points[0].lat - 51.5612345).abs() < 1e-7);
        assert_eq!(back[0].points[0].t, 3.5);
    }

    #[test]
    fn strava_csv() {
        let csv = b"Activity ID,Activity Date,Activity Name,Activity Type,Description,Filename\n\
            1,\"Sep 27, 2026\",Morning Run,Run,\"a, b\",activities/1.gpx\n\
            2,\"Sep 28, 2026\",Ride,Ride,,activities/2.fit.gz\n";
        let types = activity_types(csv);
        assert_eq!(types.get("activities/1.gpx"), Some(&(1, true)));
        assert_eq!(types.get("activities/2.fit.gz"), Some(&(2, false)));
    }
}

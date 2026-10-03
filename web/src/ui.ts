import { Popup, type Map as MlMap, type MapGeoJSONFeature } from 'maplibre-gl';
import type { ElevationRange, GradientStop, HillshadeMethod, MapConfig } from './config';
import {
  applyHillshade,
  elevationRange,
  LAYER_PREFIX,
  setTerrain,
  setTilt,
  type ContourMeta,
  type LayerGroup,
} from './layers';

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string) {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text) e.textContent = text;
  return e;
}

/** Applies each group's visibility, hiding children whose parent is off. */
function applyVisibility(map: MlMap, groups: LayerGroup[]): void {
  const byId = new Map(groups.map((g) => [g.id, g]));
  for (const g of groups) {
    const shown = g.visible && (!g.parent || byId.get(g.parent)!.visible);
    for (const id of g.layerIds) map.setLayoutProperty(id, 'visibility', shown ? 'visible' : 'none');
  }
}

/** Swatch colour shown next to a child toggle, taken from config. */
function swatchColor(cfg: MapConfig, g: LayerGroup): string | undefined {
  if (g.id.startsWith('paths-')) return cfg.paths.groups.find((s) => `paths-${s.id}` === g.id)?.color;
  if (g.id.startsWith('places-')) return cfg.places.categories[g.id.slice('places-'.length)]?.color;
  return undefined;
}

function gradientLegend(gradient: GradientStop[], range: ElevationRange, meta: ContourMeta): HTMLElement {
  const [min, max] = elevationRange(range, meta);
  const stops = [...gradient].sort((a, b) => a.at - b.at);
  const wrap = el('div', 'legend-gradient');
  const bar = el('div', 'legend-bar');
  bar.style.background = `linear-gradient(to right, ${stops.map((s) => `${s.color} ${s.at * 100}%`).join(', ')})`;
  const labels = el('div', 'legend-scale');
  labels.append(el('span', '', `${Math.round(min)} m`), el('span', '', `${Math.round(max)} m`));
  wrap.append(bar, labels);
  return wrap;
}

function landcoverLegend(cfg: MapConfig): HTMLElement {
  const list = el('div', 'legend-fills');
  for (const c of Object.values(cfg.landcover.classes)) {
    const item = el('span', 'legend-fill');
    const sw = el('i');
    sw.style.background = c.color;
    item.append(sw, document.createTextNode(c.label));
    list.append(item);
  }
  return list;
}

function checkbox(label: string, checked: boolean, onChange: (on: boolean) => void) {
  const row = el('label', 'toggle');
  const box = el('input');
  box.type = 'checkbox';
  box.checked = checked;
  box.addEventListener('change', () => onChange(box.checked));
  row.append(box, document.createTextNode(label));
  return { row, box };
}

interface SliderOptions {
  min: number;
  max: number;
  step: number;
  value: number;
  format: (v: number) => string;
  onInput: (v: number) => void;
}

function slider(label: string, o: SliderOptions) {
  const row = el('label', 'slider');
  const input = el('input');
  input.type = 'range';
  input.min = String(o.min);
  input.max = String(o.max);
  input.step = String(o.step);
  input.value = String(o.value);
  const value = el('span', 'slider-value', o.format(o.value));
  input.addEventListener('input', () => {
    value.textContent = o.format(Number(input.value));
    o.onInput(Number(input.value));
  });
  row.append(el('span', 'slider-label', label), input, value);
  return { row, input };
}

const COMPASS = ['N', 'NE', 'E', 'SE', 'S', 'SW', 'W', 'NW'];
const compass = (deg: number) => `${COMPASS[Math.round(deg / 45) % 8]}`;

/** Tilted view and 3D terrain are independent: either can be on without the other. */
function viewControls(map: MlMap, cfg: MapConfig): HTMLElement {
  const wrap = el('div', 'section');
  const tilt = checkbox('Tilted view', cfg.camera.tilted, (on) => setTilt(map, cfg, on));
  // Keep the checkbox in sync when the user tilts with the mouse.
  map.on('pitchend', () => (tilt.box.checked = map.getPitch() > 1));

  const t = cfg.terrain;
  let exaggeration = t.exaggeration;
  const height = slider('Height', {
    ...t.exaggerationSlider,
    value: t.exaggeration,
    format: (v) => `×${v}`,
    onInput: (v) => {
      exaggeration = v;
      if (terrain.box.checked) setTerrain(map, true, v);
    },
  });
  height.input.disabled = !t.enabled;
  const terrain = checkbox('3D terrain', t.enabled, (on) => {
    height.input.disabled = !on;
    setTerrain(map, on, exaggeration);
  });

  wrap.append(tilt.row, terrain.row, height.row);
  return wrap;
}

/** Light direction, intensity, light height and method for the hillshade layer. */
function hillshadeControls(map: MlMap, cfg: MapConfig): HTMLElement {
  const hs = { ...cfg.hillshade };
  const update = () => applyHillshade(map, hs);
  const wrap = el('div', 'settings');

  const method = el('select');
  for (const m of ['standard', 'basic', 'combined', 'igor', 'multidirectional'] as const) {
    const opt = el('option', '', m[0].toUpperCase() + m.slice(1));
    opt.value = m;
    opt.selected = m === hs.method;
    method.append(opt);
  }
  const methodRow = el('label', 'slider');
  methodRow.append(el('span', 'slider-label', 'Method'), method);

  const direction = slider('Light from', {
    min: 0, max: 359, step: 1, value: hs.direction,
    format: (v) => `${compass(v)} ${v}°`,
    onInput: (v) => { hs.direction = v; update(); },
  });
  const intensity = slider('Intensity', {
    min: 0, max: 1, step: 0.05, value: hs.exaggeration,
    format: (v) => v.toFixed(2),
    onInput: (v) => { hs.exaggeration = v; update(); },
  });
  const altitude = slider('Light height', {
    min: 1, max: 90, step: 1, value: hs.altitude,
    format: (v) => `${v}°`,
    onInput: (v) => { hs.altitude = v; update(); },
  });
  // The 'standard' method ignores the light altitude.
  const syncAltitude = () => (altitude.input.disabled = hs.method === 'standard');
  syncAltitude();
  method.addEventListener('change', () => {
    hs.method = method.value as HillshadeMethod;
    syncAltitude();
    update();
  });

  wrap.append(methodRow, direction.row, intensity.row, altitude.row);
  return wrap;
}

export function buildPanel(map: MlMap, cfg: MapConfig, meta: ContourMeta, groups: LayerGroup[]): void {
  const panel = el('aside', 'panel');
  const header = el('button', 'panel-header');
  header.type = 'button';
  header.append(el('h1', '', 'heathmap'), el('span', 'panel-sub', 'Hampstead Heath'));
  header.addEventListener('click', () => panel.classList.toggle('collapsed'));
  panel.append(header);
  const body = el('div', 'panel-body');
  panel.append(body);
  body.append(viewControls(map, cfg));

  // Show all / Hide all
  const boxes = new Map<LayerGroup, HTMLInputElement>();
  const setAll = (on: boolean) => {
    for (const [g, box] of boxes) {
      g.visible = on;
      box.checked = on;
    }
    applyVisibility(map, groups);
  };
  const bulk = el('div', 'bulk');
  const showAll = el('button', '', 'Show all');
  const hideAll = el('button', '', 'Hide all');
  showAll.type = hideAll.type = 'button';
  showAll.addEventListener('click', () => setAll(true));
  hideAll.addEventListener('click', () => setAll(false));
  bulk.append(el('span', 'bulk-label', 'Layers'), showAll, hideAll);
  body.append(bulk);

  for (const g of groups) {
    const row = el('label', g.parent ? 'toggle child' : 'toggle');
    const box = el('input');
    box.type = 'checkbox';
    box.checked = g.visible;
    box.addEventListener('change', () => {
      g.visible = box.checked;
      applyVisibility(map, groups);
    });
    boxes.set(g, box);
    row.append(box);
    const color = swatchColor(cfg, g);
    if (color) {
      const sw = el('i', 'swatch');
      sw.style.background = color;
      row.append(sw);
    }
    row.append(document.createTextNode(g.label));
    body.append(row);
    if (g.id === 'contours') body.append(gradientLegend(cfg.contours.gradient, cfg.contours.range, meta));
    if (g.id === 'elevation-fill') body.append(gradientLegend(cfg.elevationFill.gradient, cfg.elevationFill.range, meta));
    if (g.id === 'hillshade') body.append(hillshadeControls(map, cfg));
    if (g.id === 'landcover') body.append(landcoverLegend(cfg));
  }

  document.body.append(panel);
  applyVisibility(map, groups);
}

export function attachPopups(map: MlMap, cfg: MapConfig, groups: LayerGroup[]): void {
  const clickable = groups
    .flatMap((g) => g.layerIds)
    .filter((id) => id.startsWith(`${LAYER_PREFIX}places-`) || id === `${LAYER_PREFIX}amenities`);

  map.on('click', (e) => {
    const f: MapGeoJSONFeature | undefined = map.queryRenderedFeatures(e.point, { layers: clickable })[0];
    if (!f) return;
    const p = f.properties as Record<string, string | number>;
    const box = el('div', 'popup');
    if (f.layer.id === `${LAYER_PREFIX}amenities`) {
      box.append(el('strong', '', String(p.name ?? cfg.amenities.types[p.amenity]?.label ?? p.amenity)));
    } else {
      box.append(el('strong', '', String(p.name)));
      const cat = cfg.places.categories[p.category];
      box.append(el('div', 'popup-meta', `${cat?.label ?? p.category} · ${p.ele} m`));
      if (p.description) box.append(el('p', '', String(p.description)));
    }
    new Popup({ offset: 8, maxWidth: '260px' })
      .setLngLat((f.geometry as { coordinates: [number, number] }).coordinates)
      .setDOMContent(box)
      .addTo(map);
  });
  for (const id of clickable) {
    map.on('mouseenter', id, () => (map.getCanvas().style.cursor = 'pointer'));
    map.on('mouseleave', id, () => (map.getCanvas().style.cursor = ''));
  }
}

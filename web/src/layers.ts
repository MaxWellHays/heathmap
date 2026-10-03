import type {
  ExpressionSpecification,
  FilterSpecification,
  LayerSpecification,
  Map as MlMap,
} from 'maplibre-gl';
import type { ElevationRange, GradientStop, HillshadeConfig, MapConfig } from './config';

export interface ContourMeta {
  interval: number;
  minEle: number;
  maxEle: number;
}

/** A toggleable group of map layers, shown as one checkbox in the panel. */
export interface LayerGroup {
  id: string;
  label: string;
  layerIds: string[];
  visible: boolean;
  /** Child groups are drawn only while their parent is visible too. */
  parent?: string;
}

const DATA_FILES = ['contours', 'paths', 'landcover', 'water', 'parks', 'places', 'amenities'] as const;

export function dataUrl(name: string): string {
  return `${import.meta.env.BASE_URL}data/${name}`;
}

export interface TerrainMeta {
  minzoom: number;
  maxzoom: number;
  tileSize: number;
  encoding: 'terrarium' | 'mapbox';
  bounds: [number, number, number, number];
}

export function elevationRange(range: ElevationRange, meta: ContourMeta): [number, number] {
  return range === 'data' ? [meta.minEle, meta.maxEle] : range;
}

/** Colour as a function of elevation, mapping the gradient's 0–1 stops onto the elevation range. */
function heightColor(
  input: ExpressionSpecification,
  gradient: GradientStop[],
  range: ElevationRange,
  meta: ContourMeta,
): ExpressionSpecification {
  const [min, max] = elevationRange(range, meta);
  const stops = [...gradient].sort((a, b) => a.at - b.at);
  return ['interpolate', ['linear'], input, ...stops.flatMap((s) => [min + s.at * (max - min), s.color])];
}

const TERRAIN_SOURCE = 'dem-terrain';
export const LAYER_PREFIX = 'hm-';
const HILLSHADE_LAYER = `${LAYER_PREFIX}hillshade`;

/** Switches 3D terrain on or off. The camera tilt is controlled separately (see setTilt). */
export function setTerrain(map: MlMap, enabled: boolean, exaggeration: number): void {
  map.setTerrain(enabled ? { source: TERRAIN_SOURCE, exaggeration } : null);
}

/** Tilts the camera to the configured pitch, or back to looking straight down. */
export function setTilt(map: MlMap, cfg: MapConfig, tilted: boolean): void {
  map.easeTo({ pitch: tilted ? cfg.camera.pitch : 0, duration: 800 });
}

/** Paint properties for the hillshade layer; also used to update it live from the panel. */
export function hillshadePaint(hs: HillshadeConfig) {
  const multi = hs.method === 'multidirectional';
  const directions = multi
    ? [-1, 0, 1].map((k) => (((hs.direction + k * hs.multidirectionalSpread) % 360) + 360) % 360)
    : [hs.direction];
  const n = directions.length;
  return {
    'hillshade-method': hs.method,
    'hillshade-illumination-direction': multi ? directions : directions[0],
    'hillshade-illumination-altitude': multi ? Array(n).fill(hs.altitude) : hs.altitude,
    'hillshade-illumination-anchor': hs.anchor,
    'hillshade-exaggeration': hs.exaggeration,
    'hillshade-shadow-color': multi ? Array(n).fill(hs.shadowColor) : hs.shadowColor,
    'hillshade-highlight-color': multi ? Array(n).fill(hs.highlightColor) : hs.highlightColor,
    'hillshade-accent-color': hs.accentColor,
  } as const;
}

export function applyHillshade(map: MlMap, hs: HillshadeConfig): void {
  for (const [prop, value] of Object.entries(hillshadePaint(hs))) {
    map.setPaintProperty(HILLSHADE_LAYER, prop, value);
  }
}

/** `match` expression mapping a string property to colours, with a fallback. */
function matchColor(prop: string, colors: Record<string, string>, fallback: string): ExpressionSpecification {
  return ['match', ['get', prop], ...Object.entries(colors).flat(), fallback] as unknown as ExpressionSpecification;
}

const zoomWidth = (z14: number, z18: number): ExpressionSpecification => [
  'interpolate', ['exponential', 1.5], ['zoom'], 14, z14, 18, z18,
];

/** Id of the basemap's first label layer, so our fills and lines go underneath street names. */
function firstSymbolLayer(map: MlMap): string | undefined {
  return map.getStyle().layers.find((l) => l.type === 'symbol')?.id;
}

export function addLayers(map: MlMap, cfg: MapConfig, meta: ContourMeta, terrain: TerrainMeta): LayerGroup[] {
  for (const name of DATA_FILES) {
    map.addSource(name, { type: 'geojson', data: dataUrl(`${name}.geojson`) });
  }
  // Tile URLs must be absolute: they are resolved inside a web worker.
  const demSource = {
    type: 'raster-dem' as const,
    tiles: [`${location.origin}${dataUrl('terrain/{z}/{x}/{y}.png')}`],
    ...terrain,
  };
  // MapLibre recommends separate sources for terrain and for hillshade/colour layers.
  map.addSource('dem', demSource);
  map.addSource(TERRAIN_SOURCE, demSource);

  const groups: LayerGroup[] = [];
  const below = firstSymbolLayer(map);
  // Our layer ids get a prefix so they can't collide with the basemap's (it has its own 'water').
  // Everything except labels goes below the basemap's first label layer. Besides keeping
  // street names readable, this matters a lot for 3D: MapLibre drapes each uninterrupted
  // run of non-label layers onto the terrain in one pass, and a line layer placed above
  // the labels costs an extra pass per terrain tile (measured: 31 ms → 13 ms per frame).
  const add = (group: LayerGroup, layer: LayerSpecification) => {
    const id = `${LAYER_PREFIX}${layer.id}`;
    map.addLayer({ ...layer, id }, layer.type === 'symbol' ? undefined : below);
    group.layerIds.push(id);
  };
  const group = (id: string, label: string, visible: boolean, parent?: string): LayerGroup => {
    const g: LayerGroup = { id, label, layerIds: [], visible, parent };
    groups.push(g);
    return g;
  };

  // Elevation fill and hillshade sit at the bottom of our layers, under landcover.
  const ef = cfg.elevationFill;
  const elevationFill = group('elevation-fill', 'Elevation colours', ef.visible);
  add(elevationFill, {
    id: 'elevation-fill',
    type: 'color-relief',
    source: 'dem',
    paint: {
      'color-relief-color': heightColor(['elevation'], ef.gradient, ef.range, meta),
      'color-relief-opacity': ef.opacity,
    },
  });

  const hs = cfg.hillshade;
  const hillshade = group('hillshade', 'Hillshade', hs.visible);
  add(hillshade, {
    id: 'hillshade',
    type: 'hillshade',
    source: 'dem',
    paint: hillshadePaint(hs),
  });

  const lc = cfg.landcover;
  const landcover = group('landcover', 'Woods & fields', lc.visible);
  add(landcover, {
    id: 'landcover',
    type: 'fill',
    source: 'landcover',
    paint: {
      'fill-color': matchColor('class', Object.fromEntries(Object.entries(lc.classes).map(([k, v]) => [k, v.color])), 'rgba(0,0,0,0)'),
      'fill-opacity': lc.opacity,
    },
  });

  const water = group('water', 'Water', cfg.water.visible);
  add(water, {
    id: 'water',
    type: 'fill',
    source: 'water',
    paint: { 'fill-color': cfg.water.color, 'fill-outline-color': cfg.water.outlineColor },
  });

  const parks = group('parks', 'Park boundaries', cfg.parks.visible);
  add(parks, {
    id: 'parks',
    type: 'line',
    source: 'parks',
    paint: {
      'line-color': cfg.parks.outlineColor,
      'line-width': cfg.parks.outlineWidth,
      'line-dasharray': cfg.parks.dash,
    },
  });

  // Contours: minor lines, major lines, then labels along the major lines.
  const c = cfg.contours;
  const color = heightColor(['get', 'ele'], c.gradient, c.range, meta);
  const isMajor: ExpressionSpecification = ['==', ['%', ['get', 'ele'], c.majorInterval], 0];
  const isMinor: ExpressionSpecification = ['all', ['==', ['%', ['get', 'ele'], c.minorInterval], 0], ['!', isMajor]];
  const contours = group('contours', 'Contours', c.visible);
  add(contours, {
    id: 'contours-minor',
    type: 'line',
    source: 'contours',
    minzoom: c.minorMinZoom,
    filter: isMinor as FilterSpecification,
    paint: { 'line-color': color, 'line-width': c.minorWidth, 'line-opacity': c.opacity },
  });
  add(contours, {
    id: 'contours-major',
    type: 'line',
    source: 'contours',
    filter: isMajor as FilterSpecification,
    paint: { 'line-color': color, 'line-width': c.majorWidth, 'line-opacity': c.opacity },
  });
  if (c.labels.visible) {
    add(contours, {
      id: 'contours-labels',
      type: 'symbol',
      source: 'contours',
      minzoom: c.labels.minZoom,
      filter: isMajor as FilterSpecification,
      layout: {
        'symbol-placement': 'line',
        'text-field': ['concat', ['to-string', ['get', 'ele']], ' m'],
        'text-font': cfg.map.font,
        'text-size': c.labels.size,
        'symbol-spacing': 350,
      },
      paint: { 'text-color': color, 'text-halo-color': c.labels.haloColor, 'text-halo-width': 1.5 },
    });
  }

  // Paths: one layer per surface group so each can have its own dash pattern and toggle.
  const p = cfg.paths;
  const listed = p.groups.flatMap((g) => g.surfaces);
  const paths = group('paths', 'Footpaths', p.visible);
  add(paths, {
    id: 'paths-casing',
    type: 'line',
    source: 'paths',
    layout: { 'line-cap': 'round', 'line-join': 'round' },
    paint: { 'line-color': p.casingColor, 'line-width': zoomWidth(p.width[0] + 1.5, p.width[1] + 2), 'line-opacity': 0.8 },
  });
  for (const sg of p.groups) {
    const filter: FilterSpecification =
      sg.id === p.fallbackGroup
        ? ['!', ['in', ['get', 'surface'], ['literal', listed]]]
        : ['in', ['get', 'surface'], ['literal', sg.surfaces]];
    const surfaceGroup = group(`paths-${sg.id}`, sg.label, sg.visible, 'paths');
    add(surfaceGroup, {
      id: `paths-${sg.id}`,
      type: 'line',
      source: 'paths',
      filter,
      layout: { 'line-cap': sg.dash ? 'butt' : 'round', 'line-join': 'round' },
      paint: {
        'line-color': sg.color,
        'line-width': zoomWidth(...p.width),
        ...(sg.dash ? { 'line-dasharray': sg.dash } : {}),
      },
    });
  }

  // Amenities: small dots, only when zoomed in.
  const a = cfg.amenities;
  const amenities = group('amenities', 'Cafés, toilets, water', a.visible);
  add(amenities, {
    id: 'amenities',
    type: 'circle',
    source: 'amenities',
    minzoom: a.minZoom,
    paint: {
      'circle-radius': 3.5,
      'circle-color': matchColor('amenity', Object.fromEntries(Object.entries(a.types).map(([k, v]) => [k, v.color])), '#888'),
      'circle-stroke-color': '#fff',
      'circle-stroke-width': 1,
    },
  });

  // Places: one dot + label layer pair per category.
  group('places', 'Places', true);
  for (const [cat, pc] of Object.entries(cfg.places.categories)) {
    const g = group(`places-${cat}`, pc.label, pc.visible, 'places');
    const filter: FilterSpecification = ['==', ['get', 'category'], cat];
    if (pc.radius > 0) {
      add(g, {
        id: `places-${cat}-dot`,
        type: 'circle',
        source: 'places',
        minzoom: pc.minZoom,
        filter,
        paint: {
          'circle-radius': pc.radius,
          'circle-color': pc.color,
          'circle-stroke-color': '#fff',
          'circle-stroke-width': 1.5,
        },
      });
    }
    add(g, {
      id: `places-${cat}-label`,
      type: 'symbol',
      source: 'places',
      minzoom: pc.minZoom,
      filter,
      layout: {
        'text-field': pc.showElevation
          ? ['format', ['get', 'name'], {}, '\n', {}, ['concat', ['to-string', ['get', 'ele']], ' m'], { 'font-scale': 0.85 }]
          : ['get', 'name'],
        'text-font': pc.radius > 0 ? cfg.map.fontBold : cfg.map.font,
        'text-size': pc.textSize,
        'text-offset': pc.radius > 0 ? [0, 0.9] : [0, 0],
        'text-anchor': pc.radius > 0 ? 'top' : 'center',
        'text-max-width': 9,
        ...(pc.radius > 0 ? {} : { 'text-letter-spacing': 0.1, 'text-transform': 'uppercase' as const }),
      },
      paint: {
        'text-color': pc.color,
        'text-halo-color': '#ffffff',
        'text-halo-width': 1.5,
      },
    });
  }

  return groups;
}

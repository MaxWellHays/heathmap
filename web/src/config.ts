/**
 * All tunable settings for the map live here. Layers, colours, widths, zoom
 * thresholds and groupings are read from this object; nothing visual is
 * hard-coded in the rendering code.
 */

export type Color = string;

/** A colour gradient: `at` runs from 0 (lowest elevation) to 1 (highest). */
export interface GradientStop {
  at: number;
  color: Color;
}

/** Elevation range mapped onto a gradient: 'data' uses the min/max of the contour data. */
export type ElevationRange = 'data' | [min: number, max: number];

export interface ContourConfig {
  visible: boolean;
  range: ElevationRange;
  gradient: GradientStop[];
  /** Draw a minor contour every N metres (multiple of the 1 m data interval). */
  minorInterval: number;
  /** Every N metres a contour is drawn as major (thicker, labelled). */
  majorInterval: number;
  minorWidth: number;
  majorWidth: number;
  opacity: number;
  /** Minor contours are hidden below this zoom to reduce clutter. */
  minorMinZoom: number;
  labels: { visible: boolean; minZoom: number; size: number; haloColor: Color };
}

export type OrbitModifier = 'ctrl' | 'shift' | 'alt' | 'meta';

export interface CameraConfig {
  /** Start with the camera tilted. Independent of 3D terrain; toggled from the panel. */
  tilted: boolean;
  /** Tilt used by the "Tilted view" toggle (degrees). */
  pitch: number;
  maxPitch: number;
  /**
   * Orbit (rotate + tilt) with the mouse, Google Maps style: hold a modifier and
   * drag with the left button, or drag with the right button. Horizontal movement
   * rotates, vertical movement tilts.
   */
  orbit: {
    modifiers: OrbitModifier[];
    rightButton: boolean;
    /** Orbit around the point under the cursor (Google Maps) instead of the screen centre. */
    aroundCursor: boolean;
    degreesPerPixel: { bearing: number; pitch: number };
  };
}

export interface TerrainConfig {
  /** Start the map with 3D terrain. Can be toggled from the panel either way. */
  enabled: boolean;
  /** Vertical exaggeration multiplier (1 = true scale). */
  exaggeration: number;
  /** Range and step of the multiplier slider in the panel. */
  exaggerationSlider: { min: number; max: number; step: number };
}

export interface ElevationFillConfig {
  visible: boolean;
  opacity: number;
  range: ElevationRange;
  gradient: GradientStop[];
}

/**
 * MapLibre shading algorithms: 'standard' (classic look), 'basic' (simple, ESRI-like),
 * 'combined' (slope + aspect), 'igor' (softer, keeps detail in shadows),
 * 'multidirectional' (light from several directions fanned around `direction`).
 */
export type HillshadeMethod = 'standard' | 'basic' | 'combined' | 'igor' | 'multidirectional';

export interface HillshadeConfig {
  visible: boolean;
  method: HillshadeMethod;
  /** Direction the light comes from, degrees clockwise from north (315 = north-west). */
  direction: number;
  /** Height of the light above the horizon, degrees (lower = longer shadows). Not used by 'standard'. */
  altitude: number;
  /** Shading intensity, 0–1. */
  exaggeration: number;
  /** Angle between the lights for 'multidirectional' (three lights: direction − spread, direction, direction + spread). */
  multidirectionalSpread: number;
  /** Light direction follows the map rotation ('viewport') or stays fixed to north ('map'). */
  anchor: 'map' | 'viewport';
  shadowColor: Color;
  highlightColor: Color;
  accentColor: Color;
}

export interface SurfaceGroup {
  id: string;
  label: string;
  /** Raw OSM `surface` values in this group; the 'unknown' group catches anything unlisted. */
  surfaces: string[];
  color: Color;
  /** Line dash pattern in line widths, or omit for a solid line. */
  dash?: number[];
  visible: boolean;
}

export interface PathConfig {
  visible: boolean;
  /** Line width at zoom 14 and zoom 18; interpolated in between. */
  width: [z14: number, z18: number];
  casingColor: Color;
  groups: SurfaceGroup[];
  /** Group id for paths whose surface isn't in any group. */
  fallbackGroup: string;
}

export interface FillClass {
  label: string;
  color: Color;
}

export interface LandcoverConfig {
  visible: boolean;
  opacity: number;
  classes: Record<string, FillClass>;
}

export interface PlaceCategory {
  label: string;
  color: Color;
  /** Show the elevation under the name. */
  showElevation: boolean;
  /** Circle radius in pixels; 0 draws a label only (used for area names). */
  radius: number;
  textSize: number;
  minZoom: number;
  visible: boolean;
}

export interface MapConfig {
  title: string;
  camera: CameraConfig;
  map: {
    center: [lon: number, lat: number];
    zoom: number;
    minZoom: number;
    maxZoom: number;
    /** Panning is limited to this box: [west, south, east, north]. */
    maxBounds: [number, number, number, number];
    /** Any MapLibre style URL; data layers are drawn on top of it. */
    basemapStyle: string;
    /** Fonts must exist on the basemap style's glyph server. */
    font: string[];
    fontBold: string[];
    attribution: string;
  };
  terrain: TerrainConfig;
  elevationFill: ElevationFillConfig;
  hillshade: HillshadeConfig;
  contours: ContourConfig;
  paths: PathConfig;
  landcover: LandcoverConfig;
  water: { visible: boolean; color: Color; outlineColor: Color };
  parks: { visible: boolean; outlineColor: Color; outlineWidth: number; dash: number[] };
  places: { categories: Record<string, PlaceCategory> };
  amenities: { visible: boolean; minZoom: number; types: Record<string, { label: string; color: Color }> };
}

/** Low = blue, high = red. Shared default for contours and the elevation fill. */
const HEIGHT_GRADIENT: GradientStop[] = [
  { at: 0, color: '#2c7bb6' },
  { at: 0.35, color: '#abd9e9' },
  { at: 0.55, color: '#fee090' },
  { at: 0.75, color: '#fc8d59' },
  { at: 1, color: '#d7191c' },
];

export const config: MapConfig = {
  title: 'heathmap — Hampstead Heath',
  map: {
    center: [-0.1675, 51.5635],
    zoom: 14.6,
    minZoom: 12, // terrain tiles start at zoom 12
    maxZoom: 19,
    // Keep inside the elevation data extent (see data/area.py), or 3D terrain runs out.
    maxBounds: [-0.215, 51.54, -0.125, 51.59],
    basemapStyle: 'https://tiles.openfreemap.org/styles/positron',
    font: ['Noto Sans Regular'],
    fontBold: ['Noto Sans Bold'],
    attribution:
      'Elevation: <a href="https://environment.data.gov.uk/dataset/13787b9a-26a4-4775-8523-806d13af58fc">Environment Agency LIDAR</a> (OGL v3)',
  },

  camera: {
    tilted: false,
    pitch: 60,
    maxPitch: 80,
    orbit: {
      modifiers: ['ctrl', 'shift'], // Ctrl+drag as in Google Maps, Shift+drag as in Google Earth
      rightButton: true,
      aroundCursor: true,
      degreesPerPixel: { bearing: 0.4, pitch: 0.3 },
    },
  },

  terrain: {
    enabled: false,
    exaggeration: 2,
    exaggerationSlider: { min: 1, max: 6, step: 0.5 },
  },

  elevationFill: {
    visible: false,
    opacity: 0.55,
    range: 'data',
    gradient: HEIGHT_GRADIENT,
  },

  hillshade: {
    visible: true,
    method: 'igor',
    direction: 315,
    altitude: 45,
    exaggeration: 0.5,
    multidirectionalSpread: 45,
    anchor: 'map',
    shadowColor: '#3d4a3d',
    highlightColor: '#ffffff',
    accentColor: '#5a6b5a',
  },

  contours: {
    visible: true,
    range: 'data',
    gradient: HEIGHT_GRADIENT,
    minorInterval: 2,
    majorInterval: 10,
    minorWidth: 0.6,
    majorWidth: 1.6,
    opacity: 0.85,
    minorMinZoom: 14,
    labels: { visible: true, minZoom: 14.5, size: 11, haloColor: '#ffffff' },
  },

  paths: {
    visible: true,
    width: [1.2, 4],
    casingColor: '#ffffff',
    fallbackGroup: 'unknown',
    groups: [
      {
        id: 'paved',
        label: 'Paved',
        surfaces: ['asphalt', 'paved', 'concrete', 'concrete:plates', 'concrete:lanes', 'paving_stones', 'sett', 'crazy_paving', 'chipseal'],
        color: '#4d4d4d',
        visible: true,
      },
      {
        id: 'gravel',
        label: 'Gravel',
        surfaces: ['compacted', 'gravel', 'fine_gravel', 'pebblestone'],
        color: '#c98a2b',
        visible: true,
      },
      {
        id: 'natural',
        label: 'Dirt / grass',
        surfaces: ['dirt', 'ground', 'earth', 'grass', 'mud', 'unpaved', 'sand', 'rock', 'woodchips'],
        color: '#8c510a',
        dash: [2, 1.2],
        visible: true,
      },
      {
        id: 'boardwalk',
        label: 'Boardwalk / bridge',
        surfaces: ['wood', 'metal', 'metal_grid'],
        color: '#7b3294',
        visible: true,
      },
      {
        id: 'unknown',
        label: 'Surface not mapped',
        surfaces: [],
        color: '#9e9e9e',
        dash: [1, 1],
        visible: true,
      },
    ],
  },

  landcover: {
    visible: true,
    opacity: 0.55,
    classes: {
      wood: { label: 'Woodland', color: '#5a9a52' },
      scrub: { label: 'Scrub', color: '#9cc27a' },
      heath: { label: 'Heath', color: '#c3b77a' },
      meadow: { label: 'Meadow', color: '#cfe3a1' },
      grass: { label: 'Grass', color: '#dcebc0' },
      wetland: { label: 'Wetland', color: '#a8d5c8' },
      pitch: { label: 'Pitch', color: '#b9dfa3' },
      garden: { label: 'Garden', color: '#c6e3b5' },
      playground: { label: 'Playground', color: '#f2d9b5' },
      cemetery: { label: 'Cemetery', color: '#b7c9b0' },
      allotments: { label: 'Allotments', color: '#e0d4a8' },
    },
  },

  water: { visible: true, color: '#9fcbe8', outlineColor: '#5d9cc9' },
  parks: { visible: true, outlineColor: '#2f6b2f', outlineWidth: 1.5, dash: [3, 2] },

  places: {
    categories: {
      hill: { label: 'Hills', color: '#b2182b', showElevation: true, radius: 5, textSize: 13, minZoom: 12, visible: true },
      area: { label: 'Areas', color: '#33691e', showElevation: false, radius: 0, textSize: 12, minZoom: 13.5, visible: true },
      landmark: { label: 'Landmarks', color: '#5e35b1', showElevation: false, radius: 5, textSize: 12, minZoom: 13, visible: true },
      museum: { label: 'Museums', color: '#8e24aa', showElevation: false, radius: 4.5, textSize: 11, minZoom: 14, visible: true },
      swimming: { label: 'Swimming', color: '#0277bd', showElevation: false, radius: 4.5, textSize: 11, minZoom: 14, visible: true },
      running: { label: 'Running', color: '#e65100', showElevation: false, radius: 4.5, textSize: 11, minZoom: 14, visible: true },
      pub: { label: 'Pubs', color: '#795548', showElevation: false, radius: 4.5, textSize: 11, minZoom: 14, visible: true },
    },
  },

  amenities: {
    visible: true,
    minZoom: 15.5,
    types: {
      cafe: { label: 'Café', color: '#6d4c41' },
      toilets: { label: 'Toilets', color: '#546e7a' },
      drinking_water: { label: 'Drinking water', color: '#0288d1' },
    },
  },
};

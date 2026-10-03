import { GeolocateControl, Map, NavigationControl, ScaleControl } from 'maplibre-gl';
import 'maplibre-gl/dist/maplibre-gl.css';
import './style.css';
import { config } from './config';
import { installOrbitControls } from './camera';
import { addLayers, dataUrl, setTerrain, type ContourMeta, type TerrainMeta } from './layers';
import { attachPopups, buildPanel } from './ui';

document.title = config.title;

const map = new Map({
  container: 'map',
  style: config.map.basemapStyle,
  center: config.map.center,
  zoom: config.map.zoom,
  minZoom: config.map.minZoom,
  maxZoom: config.map.maxZoom,
  maxBounds: config.map.maxBounds,
  pitch: config.camera.tilted ? config.camera.pitch : 0,
  maxPitch: config.camera.maxPitch,
  attributionControl: { compact: true, customAttribution: config.map.attribution },
});
if (import.meta.env.DEV) Object.assign(window, { map }); // handy for debugging in the console

installOrbitControls(map, config.camera);

map.addControl(new NavigationControl(), 'top-right');
map.addControl(new ScaleControl({ unit: 'metric' }), 'bottom-left');
map.addControl(new GeolocateControl({ trackUserLocation: true }), 'top-right');

const fetchJson = <T>(name: string): Promise<T> => fetch(dataUrl(name)).then((r) => r.json());
const metaPromise = Promise.all([
  fetchJson<ContourMeta>('contours.meta.json'),
  fetchJson<TerrainMeta>('terrain.meta.json'),
]);

map.on('load', async () => {
  const [meta, terrainMeta] = await metaPromise;
  const groups = addLayers(map, config, meta, terrainMeta);
  if (config.terrain.enabled) setTerrain(map, true, config.terrain.exaggeration);
  buildPanel(map, config, meta, groups);
  attachPopups(map, config, groups);
});

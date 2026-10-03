import type { LngLat, Map as MlMap, PointLike } from 'maplibre-gl';
import type { CameraConfig, OrbitModifier } from './config';

const MODIFIER_KEYS: Record<OrbitModifier, keyof MouseEvent> = {
  ctrl: 'ctrlKey',
  shift: 'shiftKey',
  alt: 'altKey',
  meta: 'metaKey',
};

/**
 * Google Maps-style mouse orbit: modifier+left drag (or right drag) rotates with
 * horizontal movement and tilts with vertical movement, pivoting around the point
 * that was under the cursor when the drag started.
 *
 * Replaces MapLibre's built-in drag-rotate (which always pivots around the screen
 * centre) and box-zoom (which would otherwise grab Shift+drag). Touch gestures and
 * scroll zoom (already cursor-centred, like Google's) are left as they are.
 */
export function installOrbitControls(map: MlMap, camera: CameraConfig): void {
  const orbit = camera.orbit;
  map.dragRotate.disable();
  if (orbit.modifiers.includes('shift')) map.boxZoom.disable();

  const container = map.getContainer();
  const canvasContainer = map.getCanvasContainer();
  let drag: { x: number; y: number; bearing: number; pitch: number; pivot?: LngLat; pivotPx: PointLike } | null =
    null;

  const isOrbitStart = (e: MouseEvent) =>
    (e.button === 2 && orbit.rightButton) ||
    (e.button === 0 && orbit.modifiers.some((m) => e[MODIFIER_KEYS[m]]));

  const onMove = (e: MouseEvent) => {
    if (!drag) return;
    const dx = e.clientX - drag.x;
    const dy = e.clientY - drag.y;
    map.jumpTo({
      bearing: drag.bearing - dx * orbit.degreesPerPixel.bearing,
      pitch: drag.pitch - dy * orbit.degreesPerPixel.pitch,
    });
    if (drag.pivot) {
      // Pan so the pivot point stays where it was on screen.
      const now = map.project(drag.pivot);
      const [px, py] = drag.pivotPx as [number, number];
      map.panBy([now.x - px, now.y - py], { animate: false });
    }
  };

  const onUp = () => {
    drag = null;
    canvasContainer.style.cursor = '';
    window.removeEventListener('mousemove', onMove);
    window.removeEventListener('mouseup', onUp);
  };

  // Capture phase on an ancestor, so MapLibre's own pan handler never sees orbit drags.
  container.addEventListener(
    'mousedown',
    (e) => {
      if (!isOrbitStart(e) || !canvasContainer.contains(e.target as Node)) return;
      e.preventDefault();
      e.stopPropagation();
      const rect = canvasContainer.getBoundingClientRect();
      const point: [number, number] = [e.clientX - rect.left, e.clientY - rect.top];
      drag = {
        x: e.clientX,
        y: e.clientY,
        bearing: map.getBearing(),
        pitch: map.getPitch(),
        pivot: orbit.aroundCursor ? map.unproject(point) : undefined,
        pivotPx: point,
      };
      canvasContainer.style.cursor = 'move';
      window.addEventListener('mousemove', onMove);
      window.addEventListener('mouseup', onUp);
    },
    { capture: true },
  );

  if (orbit.rightButton) canvasContainer.addEventListener('contextmenu', (e) => e.preventDefault());
}

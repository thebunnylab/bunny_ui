// The presentation surface, and the one rule that shapes this file: a
// canvas element's context kind is fixed for its LIFE. Claim "2d" and
// webgl2 is gone from that element forever; claim webgl2 and the CPU
// road can never blit into it again.
//
// So the page owns a WRAPPER and the surface is its CHILD, minted the
// first time a tier asks for it and swapped whole when a tier falls.
// Falling between tiers is then one call and no listener moves: every
// event is bound to the wrapper, which outlives them both.

const host = document.getElementById("app");

const surfaces = new Map();

// The element for `kind` ("2d" or "gl"), mounted. Asking for the other
// kind releases the one on screen — its context can never be reclaimed,
// so holding its pixels would be paying for a road already closed.
function surface(kind) {
  let canvas = surfaces.get(kind);
  if (!canvas) {
    canvas = document.createElement("canvas");
    canvas.style.cssText = "display:block;width:100%;height:100%;outline:none";
    surfaces.set(kind, canvas);
  }
  if (canvas.parentNode !== host) {
    for (const [held, spent] of surfaces) {
      if (held === kind) continue;
      // a zero-sized backing store is the only way to hand the memory
      // back before the element is collected
      spent.width = 0;
      spent.height = 0;
      surfaces.delete(held);
    }
    host.replaceChildren(canvas);
  }
  return canvas;
}

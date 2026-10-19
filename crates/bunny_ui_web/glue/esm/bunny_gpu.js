// The WebGL2 tier's half of the border, as an ES module.
//
// The same thin, GL-shaped door `glue_gl.js` is — one verb per GL call,
// every policy decision resolved in Rust — written so a page whose
// LOADER is someone else's can import it: wasm-bindgen resolves the
// wasm's `./bunny_gpu.js` import module as `import * as m from
// "./bunny_gpu.js"`, and every verb below is a named export of that
// module. `attach` hands this file the instance, the memory and the
// surface door once the page has them; until then every verb answers
// zero, so a worker that evaluates this module (a threaded build
// instantiates the wasm in every worker it spawns) never reaches for a
// document it does not have.
//
// GL objects live in a handle table here, because a WebGLBuffer is an
// opaque JS value and cannot cross into wasm. Index zero is null, which
// is also GL's own name for "no object".

let glWasm = null;
let glMemory = null;
let glSurface = null;
let gl = null;
let glKind = 0;
let glLastLog = "";

const glObjects = [null];
const glFree = [];

function glPut(value) {
  if (glFree.length) {
    const slot = glFree.pop();
    glObjects[slot] = value;
    return slot;
  }
  glObjects.push(value);
  return glObjects.length - 1;
}

function glObj(handle) {
  return glObjects[handle >>> 0] ?? null;
}

// the GPU's timers, where the browser keeps them: asked once a context
let glTimer = null;
let glTimerFor = null;

function glTimerExt() {
  if (glTimerFor !== gl) {
    glTimerFor = gl;
    glTimer = gl ? gl.getExtension("EXT_disjoint_timer_query_webgl2") : null;
  }
  return glTimer;
}

function glRelease(handle) {
  const slot = handle >>> 0;
  if (slot === 0 || slot >= glObjects.length) return null;
  const value = glObjects[slot];
  glObjects[slot] = null;
  glFree.push(slot);
  return value;
}

// Growing the wasm heap DETACHES every view over it, so a view is built
// per call and never cached. Under shared memory the buffer never
// detaches — and WebGL's upload and readback verbs accept a shared
// view, so no copy is needed on this side of the border.
function glHeap() {
  return new Uint8Array(glMemory.buffer);
}

function glBytes(pointer, length) {
  return new Uint8Array(glMemory.buffer, pointer >>> 0, length >>> 0);
}

// Floats for a FLOAT texture or a vec4 array: a view where the pointer
// sits on four bytes, a copy where it does not (a view would throw)
function glFloats(pointer, length) {
  pointer >>>= 0;
  length >>>= 0;
  if (pointer % 4 === 0) return new Float32Array(glMemory.buffer, pointer, length >>> 2);
  return new Float32Array(glBytes(pointer, length).slice().buffer);
}

// Words for an UNSIGNED_INT texture, the same way: WebGL takes no bytes
// for words
function glWords(pointer, length) {
  pointer >>>= 0;
  length >>>= 0;
  if (pointer % 4 === 0) return new Uint32Array(glMemory.buffer, pointer, length >>> 2);
  return new Uint32Array(glBytes(pointer, length).slice().buffer);
}

const glDecoder = new TextDecoder();
const glEncoder = new TextEncoder();

function glText(pointer, length) {
  // decode COPIES — and it refuses a shared view, so copy first there
  const view = glBytes(pointer, length);
  return glDecoder.decode(
    typeof SharedArrayBuffer !== "undefined" && view.buffer instanceof SharedArrayBuffer
      ? view.slice()
      : view,
  );
}

/// The page hands over what the verbs need: the instance (for the
/// context-loss callbacks), the memory (uploads read it) and the door
/// to the presentation surface (`surface("gl")` in bunny.js).
export function attach(exports, memory, surface) {
  glWasm = exports;
  glMemory = memory;
  glSurface = surface;
}

function gpuLoseContext() {
  // the tier is gone: every handle names an object of a dead context
  glObjects.length = 1;
  glFree.length = 0;
  gl = null;
}

// `kind` 0 is the page's own surface, 1 the islands' backing canvas.
// 0 back means refused — no WebGL2, a shader that would not compile,
// `?present=cpu`, or a page that has not attached yet. Non-zero is
// MAX_TEXTURE_SIZE, which the atlas needs before it decides how far it
// may grow.
export function gl_init(kind, width, height) {
  if (!glSurface || typeof document === "undefined") return 0;
  const forced = new URLSearchParams(location.search).get("present");
  if (forced === "cpu") return 0;
  const target =
    kind === 0
      ? glSurface("gl")
      : // never in the document: the islands draw here and each one is
        // copied into its own element
        document.createElement("canvas");
  target.width = width >>> 0 || 1;
  target.height = height >>> 0 || 1;
  target.addEventListener(
    "webglcontextlost",
    (event) => {
      // WITHOUT this the restored event never fires. It is one line
      // and there is no recovery without it.
      event.preventDefault();
      gpuLoseContext();
      if (glWasm && glWasm.bunny_gpu_lost) glWasm.bunny_gpu_lost();
    },
    false,
  );
  target.addEventListener(
    "webglcontextrestored",
    () => {
      if (glWasm && glWasm.bunny_gpu_restored) {
        glWasm.bunny_gpu_restored(target.width, target.height);
      }
    },
    false,
  );
  gl = target.getContext("webgl2", {
    // the page is opaque, so the compositor never blends the canvas
    alpha: kind !== 0,
    // what the framebuffer HOLDS after the house blend is premultiplied
    // by construction: over a transparent clear a half-covered pixel
    // lands as (0.5c, 0.5). Measured, not assumed.
    premultipliedAlpha: true,
    // MSAA would anti-alias polygon EDGES, and the coverage here is
    // analytic in the fragment shader. It would seam exactly where two
    // quads abut — the seam the parity gate measures — and cost four
    // times the fill for it.
    antialias: false,
    depth: false,
    stencil: false,
    preserveDrawingBuffer: false,
    // a software context is refused: SwiftShader rasterizes through a
    // general driver with no damage knowledge, and the rasterizer this
    // tier falls back to is specialized and repaints partially. Our own
    // floor is the better floor.
    failIfMajorPerformanceCaveat: true,
  });
  if (!gl) return 0;
  glKind = kind;
  return gl.getParameter(gl.MAX_TEXTURE_SIZE) >>> 0;
}

// the tier's one line on the way down, so a person can see WHY the
// page fell to the rasterizer
export function gl_log(pointer, length) {
  if (!glMemory) return;
  console.warn(glText(pointer, length));
}

// One island, copied out of the shared surface into its own element.
// The canvas page has no islands (that is the Dom lowering's road), so
// without an element map there is nothing to copy into.
export function gl_island_blit(id, width, height) {
  if (!gl) return;
  const element = typeof elements !== "undefined" ? elements.get(id >>> 0) : null;
  if (!element) return;
  if (element.width !== (width >>> 0) || element.height !== (height >>> 0)) {
    element.width = width >>> 0;
    element.height = height >>> 0;
  }
  const into = element.getContext("2d");
  if (!into) return;
  // without this the island ghosts its previous frame underneath
  into.globalCompositeOperation = "copy";
  into.drawImage(gl.canvas, 0, 0, width >>> 0, height >>> 0, 0, 0, width >>> 0, height >>> 0);
}

export function gl_now() {
  return performance.now();
}

export function gl_teardown() {
  gpuLoseContext();
}

export function gl_resize(width, height) {
  if (!gl) return;
  const target = gl.canvas;
  if (target.width !== (width >>> 0) || target.height !== (height >>> 0)) {
    target.width = width >>> 0;
    target.height = height >>> 0;
  }
}

// MARK: - Fixed state

export function gl_viewport(x, y, width, height) { gl.viewport(x | 0, y | 0, width | 0, height | 0); }
export function gl_scissor(x, y, width, height) { gl.scissor(x | 0, y | 0, width | 0, height | 0); }
export function gl_clear_color(r, g, b, a) { gl.clearColor(r, g, b, a); }
export function gl_clear(mask) { gl.clear(mask >>> 0); }
export function gl_enable(cap) { gl.enable(cap >>> 0); }
export function gl_disable(cap) { gl.disable(cap >>> 0); }
export function gl_blend_func_separate(sc, dc, sa, da) {
  gl.blendFuncSeparate(sc >>> 0, dc >>> 0, sa >>> 0, da >>> 0);
}
export function gl_pixel_storei(name, param) { gl.pixelStorei(name >>> 0, param | 0); }
export function gl_finish() { gl.finish(); }
export function gl_flush() { gl.flush(); }

// MARK: - Programs

export function gl_compile_shader(kind, pointer, length) {
  const shader = gl.createShader(kind >>> 0);
  gl.shaderSource(shader, glText(pointer, length));
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    glLastLog = gl.getShaderInfoLog(shader) || "";
    gl.deleteShader(shader);
    return 0;
  }
  return glPut(shader);
}

export function gl_link_program(vertex, fragment) {
  const program = gl.createProgram();
  gl.attachShader(program, glObj(vertex));
  gl.attachShader(program, glObj(fragment));
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    glLastLog = gl.getProgramInfoLog(program) || "";
    gl.deleteProgram(program);
    return 0;
  }
  return glPut(program);
}

export function gl_bind_attrib_location(program, index, pointer, length) {
  gl.bindAttribLocation(glObj(program), index >>> 0, glText(pointer, length));
}
export function gl_use_program(program) { gl.useProgram(glObj(program)); }
export function gl_uniform_location(program, pointer, length) {
  const location = gl.getUniformLocation(glObj(program), glText(pointer, length));
  // null is a uniform the linker dropped; zero writes then no-op
  return location ? glPut(location) : 0;
}
export function gl_uniform_block(program, pointer, length, binding) {
  const index = gl.getUniformBlockIndex(glObj(program), glText(pointer, length));
  if (index !== 0xffffffff) gl.uniformBlockBinding(glObj(program), index, binding >>> 0);
}
export function gl_uniform1i(location, value) { gl.uniform1i(glObj(location), value | 0); }
export function gl_uniform4f(location, x, y, z, w) { gl.uniform4f(glObj(location), x, y, z, w); }

// The last compile or link complaint, into wasm memory. Two-phase, the
// way every string crosses here.
export function gl_last_log(out, cap) {
  const encoded = glEncoder.encode(glLastLog).subarray(0, cap >>> 0);
  glBytes(out, encoded.length).set(encoded);
  return encoded.length;
}

// MARK: - Buffers

export function gl_create_buffer() { return glPut(gl.createBuffer()); }
export function gl_bind_buffer(target, buffer) { gl.bindBuffer(target >>> 0, glObj(buffer)); }
export function gl_bind_buffer_base(target, index, buffer) {
  gl.bindBufferBase(target >>> 0, index >>> 0, glObj(buffer));
}
// orphaning: a null store of the same size lets the driver rename the
// buffer instead of stalling on the frame still reading it
export function gl_buffer_data_size(target, size, usage) {
  gl.bufferData(target >>> 0, size >>> 0, usage >>> 0);
}
// the (view, offset, length) overload over the WHOLE heap: no subarray,
// no allocation, and WebGL does the bounds check
export function gl_buffer_sub_data(target, offset, pointer, length) {
  gl.bufferSubData(target >>> 0, offset >>> 0, glHeap(), pointer >>> 0, length >>> 0);
}
export function gl_delete_buffer(buffer) { gl.deleteBuffer(glRelease(buffer)); }

// MARK: - Vertex arrays

export function gl_create_vertex_array() { return glPut(gl.createVertexArray()); }
export function gl_bind_vertex_array(array) { gl.bindVertexArray(glObj(array)); }
export function gl_enable_vertex_attrib_array(index) { gl.enableVertexAttribArray(index >>> 0); }
export function gl_vertex_attrib_pointer(index, size, kind, normalized, stride, offset) {
  gl.vertexAttribPointer(
    index >>> 0, size | 0, kind >>> 0, normalized !== 0, stride | 0, offset | 0,
  );
}
export function gl_vertex_attrib_divisor(index, divisor) {
  gl.vertexAttribDivisor(index >>> 0, divisor >>> 0);
}

// MARK: - Textures

export function gl_create_texture() { return glPut(gl.createTexture()); }
export function gl_bind_texture(target, texture) { gl.bindTexture(target >>> 0, glObj(texture)); }
export function gl_active_texture(unit) { gl.activeTexture(unit >>> 0); }
export function gl_tex_parameteri(target, name, param) {
  gl.texParameteri(target >>> 0, name >>> 0, param | 0);
}
export function gl_tex_image_2d(target, level, internal, width, height, format, kind, pointer, length) {
  // a null pointer allocates without filling — the atlas and the
  // pyramid both open their storage that way. FLOAT data wants a view
  // of floats, UNSIGNED_INT one of words; every other kind takes the
  // bytes
  const words = (kind >>> 0) === 0x1405;
  const source = pointer
    ? (kind >>> 0) === 0x1406 ? glFloats(pointer, length) : words ? glWords(pointer, length) : glBytes(pointer, length)
    : null;
  gl.texImage2D(
    target >>> 0, level | 0, internal | 0, width | 0, height | 0, 0,
    format >>> 0, kind >>> 0, source,
  );
}
export function gl_tex_sub_image_2d(target, level, x, y, width, height, format, kind, pointer, length) {
  gl.texSubImage2D(
    target >>> 0, level | 0, x | 0, y | 0, width | 0, height | 0,
    format >>> 0, kind >>> 0, glBytes(pointer, length),
  );
}
export function gl_delete_texture(texture) { gl.deleteTexture(glRelease(texture)); }

// MARK: - Framebuffers

export function gl_create_framebuffer() { return glPut(gl.createFramebuffer()); }
export function gl_bind_framebuffer(target, framebuffer) {
  gl.bindFramebuffer(target >>> 0, glObj(framebuffer));
}
export function gl_framebuffer_texture_2d(target, attachment, textarget, texture, level) {
  gl.framebufferTexture2D(
    target >>> 0, attachment >>> 0, textarget >>> 0, glObj(texture), level | 0,
  );
}
export function gl_check_framebuffer_status(target) {
  return gl.checkFramebufferStatus(target >>> 0) >>> 0;
}
export function gl_delete_framebuffer(framebuffer) { gl.deleteFramebuffer(glRelease(framebuffer)); }

// MARK: - Draw and read

// MARK: - The world pass (feature `3d`): depth, samples, indexed meshes

export function gl_uniform4fv(location, pointer, count) {
  if (!gl) return;
  gl.uniform4fv(glObj(location), glFloats(pointer, (count >>> 0) * 16));
}
export function gl_buffer_data(target, pointer, length, usage) {
  if (!gl) return;
  gl.bufferData(target >>> 0, glHeap(), usage >>> 0, pointer >>> 0, length >>> 0);
}
export function gl_delete_vertex_array(array) {
  if (!gl) return;
  gl.deleteVertexArray(glRelease(array));
}
export function gl_vertex_attrib_i_pointer(index, size, kind, stride, offset) {
  if (!gl) return;
  gl.vertexAttribIPointer(index >>> 0, size | 0, kind >>> 0, stride | 0, offset | 0);
}
export function gl_framebuffer_renderbuffer(target, attachment, renderbuffer) {
  if (!gl) return;
  gl.framebufferRenderbuffer(target >>> 0, attachment >>> 0, gl.RENDERBUFFER, glObj(renderbuffer));
}
export function gl_create_renderbuffer() {
  if (!gl) return 0;
  return glPut(gl.createRenderbuffer());
}
export function gl_delete_renderbuffer(renderbuffer) {
  if (!gl) return;
  gl.deleteRenderbuffer(glRelease(renderbuffer));
}
// binds, then gives storage; zero samples is one
export function gl_renderbuffer_storage_multisample(renderbuffer, samples, internal, width, height) {
  if (!gl) return;
  gl.bindRenderbuffer(gl.RENDERBUFFER, glObj(renderbuffer));
  if ((samples | 0) > 0) {
    gl.renderbufferStorageMultisample(gl.RENDERBUFFER, samples | 0, internal >>> 0, width | 0, height | 0);
  } else {
    gl.renderbufferStorage(gl.RENDERBUFFER, internal >>> 0, width | 0, height | 0);
  }
}
// a rect of the read framebuffer's colour onto the draw one, its corner at
// (toX, toY): the samples' resolve, and a picture's slide
export function gl_blit_framebuffer(x, y, width, height, toX, toY) {
  if (!gl) return;
  const [x0, y0, x1, y1] = [x | 0, y | 0, (x + width) | 0, (y + height) | 0];
  const [u0, v0] = [toX | 0, toY | 0];
  gl.blitFramebuffer(x0, y0, x1, y1, u0, v0, u0 + (width | 0), v0 + (height | 0), gl.COLOR_BUFFER_BIT, gl.NEAREST);
}
// a tiled GPU need not write back what nobody reads
export function gl_invalidate_framebuffer(target, first, second) {
  if (!gl) return;
  const attachments = [];
  if (first >>> 0) attachments.push(first >>> 0);
  if (second >>> 0) attachments.push(second >>> 0);
  if (attachments.length) gl.invalidateFramebuffer(target >>> 0, attachments);
}
// the GPU's time over a run of commands, where the browser keeps it
export function gl_has_timers() {
  return glTimerExt() ? 1 : 0;
}
export function gl_timer_begin() {
  const ext = glTimerExt();
  if (!ext) return 0;
  const query = gl.createQuery();
  gl.beginQuery(ext.TIME_ELAPSED_EXT, query);
  return glPut(query);
}
export function gl_timer_end() {
  const ext = glTimerExt();
  if (ext) gl.endQuery(ext.TIME_ELAPSED_EXT);
}
// nanoseconds; -1 while the GPU runs it, -2 when the measure was lost
export function gl_timer_read(handle) {
  const query = glObj(handle);
  const ext = glTimerExt();
  if (!query || !ext) return -2;
  if (!gl.getQueryParameter(query, gl.QUERY_RESULT_AVAILABLE)) return -1;
  const lost = gl.getParameter(ext.GPU_DISJOINT_EXT);
  const nanos = gl.getQueryParameter(query, gl.QUERY_RESULT);
  gl.deleteQuery(glRelease(handle));
  return lost ? -2 : nanos;
}
// a frame that draws its glow draws into two attachments
export function gl_draw_buffers(count) {
  if (!gl) return;
  gl.drawBuffers([gl.COLOR_ATTACHMENT0, gl.COLOR_ATTACHMENT1].slice(0, Math.min(count >>> 0, 2)));
}
export function gl_read_buffer(attachment) {
  if (!gl) return;
  gl.readBuffer(attachment >>> 0);
}
export function gl_clear_bufferfv(index, r, g, b, a) {
  if (!gl) return;
  gl.clearBufferfv(gl.COLOR, index | 0, [r, g, b, a]);
}
export function gl_depth_func(func) {
  if (!gl) return;
  gl.depthFunc(func >>> 0);
}
export function gl_depth_mask(write) {
  if (!gl) return;
  gl.depthMask(write !== 0);
}
export function gl_cull_face(face) {
  if (!gl) return;
  gl.cullFace(face >>> 0);
}
export function gl_front_face(mode) {
  if (!gl) return;
  gl.frontFace(mode >>> 0);
}
export function gl_clear_depth(depth) {
  if (!gl) return;
  gl.clearDepth(depth);
}
export function gl_draw_elements_instanced(mode, count, kind, offset, instances) {
  if (!gl) return;
  gl.drawElementsInstanced(mode >>> 0, count | 0, kind >>> 0, offset | 0, instances | 0);
}

export function gl_draw_arrays(mode, first, count) { gl.drawArrays(mode >>> 0, first | 0, count | 0); }
export function gl_draw_arrays_instanced(mode, first, count, instances) {
  gl.drawArraysInstanced(mode >>> 0, first | 0, count | 0, instances | 0);
}
// this STALLS the thread: it flushes and waits. No frame may call it;
// it is the parity harness's own sync point.
// A read the browser refuses leaves the bytes as they were: the tier's
// probe then sees a tier that cannot show its tiles, and an exception
// never unwinds through the wasm.
export function gl_read_pixels(x, y, width, height, format, kind, pointer, length) {
  try {
    gl.readPixels(
      x | 0, y | 0, width | 0, height | 0, format >>> 0, kind >>> 0,
      glHeap(), pointer >>> 0,
    );
  } catch (refused) {
    console.warn("bunny gl: the browser refused a read -", refused);
  }
}

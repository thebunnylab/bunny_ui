// The WebGL2 tier's half of the border: a thin, GL-shaped door.
//
// Thin on purpose. The house law is that every policy decision — the
// snapping, the radius clamps, the stroke thickness, the shadow reach,
// the clip stack — resolves on the CPU in Rust, and what sits below is
// a pure evaluator. A fat border that handed this file the finished
// batches and let it drive the pipeline would move the encode order
// into JavaScript, where no compiler watches it and no test can reach
// it. That is the shape that rots.
//
// So: the Rust tier owns the pipeline and calls GL, one verb at a time,
// exactly as the desktop tier calls it. The cost is crossings, and the
// crossings are cheap — a full frame is a few hundred, against a budget
// of a millisecond.
//
// GL glObjects live in a handle table here, because a WebGLBuffer is an
// opaque JS value and cannot cross into wasm. Index zero is null, which
// is also GL's own name for "no object".

let glWasm = null;
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

function glRelease(handle) {
  const slot = handle >>> 0;
  if (slot === 0 || slot >= glObjects.length) return null;
  const value = glObjects[slot];
  glObjects[slot] = null;
  glFree.push(slot);
  return value;
}

// Growing the wasm glHeap DETACHES every view over it, so a view is built
// per call and never cached. This is the one rule that makes the border
// safe; forgetting it gives silent garbage after the first growth.
function glHeap() {
  return new Uint8Array(glWasm.memory.buffer);
}

function glBytes(pointer, length) {
  return new Uint8Array(glWasm.memory.buffer, pointer >>> 0, length >>> 0);
}

const glDecoder = new TextDecoder();
const glEncoder = new TextEncoder();

function glText(pointer, length) {
  // decode COPIES, so the string outlives any later growth
  return glDecoder.decode(glBytes(pointer, length));
}

function gpuAttach(instance) {
  glWasm = instance;
}

function gpuLoseContext() {
  // the tier is gone: every handle names an object of a dead context
  glObjects.length = 1;
  glFree.length = 0;
  gl = null;
}

const bunnyGpuImports = {
  // `kind` 0 is the page's own surface, 1 the islands' backing canvas.
  // 0 back means refused — no WebGL2, a shader that would not compile,
  // or `?present=cpu`. Non-zero is MAX_TEXTURE_SIZE, which the atlas
  // needs before it decides how far it may grow.
  gl_init(kind, width, height) {
    const forced = new URLSearchParams(location.search).get("present");
    if (forced === "cpu") return 0;
    const target =
      kind === 0
        ? surface("gl")
        : (() => {
            // never in the document: the islands draw here and each
            // one is copied into its own element
            const backing = document.createElement("canvas");
            return backing;
          })();
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
  },

  // the tier's one line on the way down, so a person can see WHY the
  // page fell to the rasterizer
  gl_log(pointer, length) {
    console.warn(glText(pointer, length));
  },

  // One island, copied out of the shared surface into its own element.
  // A canvas that took webgl2 can never take "2d" again, so the islands
  // keep their 2d contexts and the TIER keeps one context for all of
  // them — which is also the only arrangement a lost context can fall
  // back from without recreating an element the engine cannot emit.
  gl_island_blit(id, width, height) {
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
  },

  gl_teardown() {
    gpuLoseContext();
  },

  gl_resize(width, height) {
    if (!gl) return;
    const target = gl.canvas;
    if (target.width !== (width >>> 0) || target.height !== (height >>> 0)) {
      target.width = width >>> 0;
      target.height = height >>> 0;
    }
  },

  // MARK: - Fixed state

  gl_viewport(x, y, width, height) { gl.viewport(x | 0, y | 0, width | 0, height | 0); },
  gl_clear_color(r, g, b, a) { gl.clearColor(r, g, b, a); },
  gl_clear(mask) { gl.clear(mask >>> 0); },
  gl_enable(cap) { gl.enable(cap >>> 0); },
  gl_disable(cap) { gl.disable(cap >>> 0); },
  gl_blend_func_separate(sc, dc, sa, da) {
    gl.blendFuncSeparate(sc >>> 0, dc >>> 0, sa >>> 0, da >>> 0);
  },
  gl_pixel_storei(name, param) { gl.pixelStorei(name >>> 0, param | 0); },
  gl_finish() { gl.finish(); },
  gl_flush() { gl.flush(); },

  // MARK: - Programs

  gl_compile_shader(kind, pointer, length) {
    const shader = gl.createShader(kind >>> 0);
    gl.shaderSource(shader, glText(pointer, length));
    gl.compileShader(shader);
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
      glLastLog = gl.getShaderInfoLog(shader) || "";
      gl.deleteShader(shader);
      return 0;
    }
    return glPut(shader);
  },

  gl_link_program(vertex, fragment) {
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
  },

  gl_bind_attrib_location(program, index, pointer, length) {
    gl.bindAttribLocation(glObj(program), index >>> 0, glText(pointer, length));
  },
  gl_use_program(program) { gl.useProgram(glObj(program)); },
  gl_uniform_location(program, pointer, length) {
    const location = gl.getUniformLocation(glObj(program), glText(pointer, length));
    // null is a uniform the linker dropped; zero writes then no-op
    return location ? glPut(location) : 0;
  },
  gl_uniform_block(program, pointer, length, binding) {
    const index = gl.getUniformBlockIndex(glObj(program), glText(pointer, length));
    if (index !== 0xffffffff) gl.uniformBlockBinding(glObj(program), index, binding >>> 0);
  },
  gl_uniform1i(location, value) { gl.uniform1i(glObj(location), value | 0); },
  gl_uniform4f(location, x, y, z, w) { gl.uniform4f(glObj(location), x, y, z, w); },

  // The last compile or link complaint, into wasm memory. Two-phase, the
  // way every string crosses here.
  gl_last_log(out, cap) {
    const encoded = glEncoder.encode(glLastLog).subarray(0, cap >>> 0);
    glBytes(out, encoded.length).set(encoded);
    return encoded.length;
  },

  // MARK: - Buffers

  gl_create_buffer() { return glPut(gl.createBuffer()); },
  gl_bind_buffer(target, buffer) { gl.bindBuffer(target >>> 0, glObj(buffer)); },
  gl_bind_buffer_base(target, index, buffer) {
    gl.bindBufferBase(target >>> 0, index >>> 0, glObj(buffer));
  },
  // orphaning: a null store of the same size lets the driver rename the
  // buffer instead of stalling on the frame still reading it
  gl_buffer_data_size(target, size, usage) {
    gl.bufferData(target >>> 0, size >>> 0, usage >>> 0);
  },
  // the (view, offset, length) overload over the WHOLE glHeap: no subarray,
  // no allocation, and WebGL does the bounds check
  gl_buffer_sub_data(target, offset, pointer, length) {
    gl.bufferSubData(target >>> 0, offset >>> 0, glHeap(), pointer >>> 0, length >>> 0);
  },
  gl_delete_buffer(buffer) { gl.deleteBuffer(glRelease(buffer)); },

  // MARK: - Vertex arrays

  gl_create_vertex_array() { return glPut(gl.createVertexArray()); },
  gl_bind_vertex_array(array) { gl.bindVertexArray(glObj(array)); },
  gl_enable_vertex_attrib_array(index) { gl.enableVertexAttribArray(index >>> 0); },
  gl_vertex_attrib_pointer(index, size, kind, normalized, stride, offset) {
    gl.vertexAttribPointer(
      index >>> 0, size | 0, kind >>> 0, normalized !== 0, stride | 0, offset | 0,
    );
  },
  gl_vertex_attrib_divisor(index, divisor) {
    gl.vertexAttribDivisor(index >>> 0, divisor >>> 0);
  },

  // MARK: - Textures

  gl_create_texture() { return glPut(gl.createTexture()); },
  gl_bind_texture(target, texture) { gl.bindTexture(target >>> 0, glObj(texture)); },
  gl_active_texture(unit) { gl.activeTexture(unit >>> 0); },
  gl_tex_parameteri(target, name, param) {
    gl.texParameteri(target >>> 0, name >>> 0, param | 0);
  },
  gl_tex_image_2d(target, level, internal, width, height, format, kind, pointer, length) {
    // a null pointer allocates without filling — the atlas and the
    // pyramid both open their storage that way
    const source = pointer ? glBytes(pointer, length) : null;
    gl.texImage2D(
      target >>> 0, level | 0, internal | 0, width | 0, height | 0, 0,
      format >>> 0, kind >>> 0, source,
    );
  },
  gl_tex_sub_image_2d(target, level, x, y, width, height, format, kind, pointer, length) {
    gl.texSubImage2D(
      target >>> 0, level | 0, x | 0, y | 0, width | 0, height | 0,
      format >>> 0, kind >>> 0, glBytes(pointer, length),
    );
  },
  gl_delete_texture(texture) { gl.deleteTexture(glRelease(texture)); },

  // MARK: - Framebuffers

  gl_create_framebuffer() { return glPut(gl.createFramebuffer()); },
  gl_bind_framebuffer(target, framebuffer) {
    gl.bindFramebuffer(target >>> 0, glObj(framebuffer));
  },
  gl_framebuffer_texture_2d(target, attachment, textarget, texture, level) {
    gl.framebufferTexture2D(
      target >>> 0, attachment >>> 0, textarget >>> 0, glObj(texture), level | 0,
    );
  },
  gl_check_framebuffer_status(target) {
    return gl.checkFramebufferStatus(target >>> 0) >>> 0;
  },
  gl_delete_framebuffer(framebuffer) { gl.deleteFramebuffer(glRelease(framebuffer)); },

  // MARK: - Draw and read

  gl_draw_arrays(mode, first, count) { gl.drawArrays(mode >>> 0, first | 0, count | 0); },
  gl_draw_arrays_instanced(mode, first, count, instances) {
    gl.drawArraysInstanced(mode >>> 0, first | 0, count | 0, instances | 0);
  },
  // this STALLS the thread: it flushes and waits. No frame may call it;
  // it is the parity harness's own sync point.
  gl_read_pixels(x, y, width, height, format, kind, pointer, length) {
    gl.readPixels(
      x | 0, y | 0, width | 0, height | 0, format >>> 0, kind >>> 0,
      glHeap(), pointer >>> 0,
    );
  },
};

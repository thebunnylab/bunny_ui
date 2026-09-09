// The fullscreen triangle the blur and the blit ride: three vertices,
// no attributes, no shared edge for the rasterizer to seam. The
// committed .spv beside this file is its baked twin; a gated test
// recompiles and byte-compares when the compiler is on the box.
#version 450

void main() {
    vec2 uv = vec2(float((gl_VertexIndex << 1) & 2), float(gl_VertexIndex & 2));
    gl_Position = vec4(uv * 2.0 - 1.0, 0.0, 1.0);
}

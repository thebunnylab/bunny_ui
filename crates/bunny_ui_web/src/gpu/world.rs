//! WebGL2's world pass: the GL family's encoder
//! (`bunny_ui::gpu::gl_world`), its verbs crossing to `glue_gl.js` one
//! call at a time, the same way the 2D pass's do.

use bunny_ui::gpu::gl_world::GlFamily;

use super::*;

/// The GL family, spoken through the page's border.
pub(super) struct WebGl;

impl GlFamily for WebGl {
    fn prelude(&self) -> &'static str {
        PRELUDE_300ES
    }

    fn program(&mut self, vertex: &str, fragment: &str, attributes: &[&str], blocks: &[(&str, u32)]) -> Result<u32, String> {
        // the world's own stages fix their locations in the source; the
        // composite shares the 2D sprite stage and its way of binding them
        let vs = compile(GL_VERTEX_SHADER, vertex);
        if vs == 0 {
            return Err(last_log());
        }
        let fs = compile(GL_FRAGMENT_SHADER, fragment);
        if fs == 0 {
            return Err(last_log());
        }
        let handle = unsafe { gl_link_program(vs, fs) };
        if handle == 0 {
            return Err(last_log());
        }
        for (index, name) in attributes.iter().enumerate() {
            unsafe { gl_bind_attrib_location(handle, index as u32, name.as_ptr(), name.len()) };
        }
        let linked = if attributes.is_empty() {
            handle
        } else {
            let again = unsafe { gl_link_program(vs, fs) };
            if again == 0 {
                return Err(last_log());
            }
            again
        };
        for (name, binding) in blocks {
            unsafe { gl_uniform_block(linked, name.as_ptr(), name.len(), *binding) };
        }
        Ok(linked)
    }

    fn uniform_location(&mut self, program: u32, name: &str) -> u32 {
        unsafe { gl_uniform_location(program, name.as_ptr(), name.len()) }
    }
    fn use_program(&mut self, program: u32) {
        unsafe { gl_use_program(program) }
    }
    fn uniform1i(&mut self, location: u32, value: i32) {
        unsafe { gl_uniform1i(location, value) }
    }
    fn uniform4fv(&mut self, location: u32, values: &[[f32; 4]]) {
        if location != 0 && !values.is_empty() {
            unsafe { gl_uniform4fv(location, values.as_ptr().cast::<f32>(), values.len()) }
        }
    }

    fn create_buffer(&mut self) -> u32 {
        unsafe { gl_create_buffer() }
    }
    fn delete_buffer(&mut self, buffer: u32) {
        unsafe { gl_delete_buffer(buffer) }
    }
    fn bind_buffer(&mut self, target: u32, buffer: u32) {
        unsafe { gl_bind_buffer(target, buffer) }
    }
    fn buffer_data(&mut self, target: u32, bytes: &[u8], usage: u32) {
        unsafe { gl_buffer_data(target, bytes.as_ptr(), bytes.len(), usage) }
    }
    fn bind_buffer_base(&mut self, target: u32, index: u32, buffer: u32) {
        unsafe { gl_bind_buffer_base(target, index, buffer) }
    }

    fn create_vertex_array(&mut self) -> u32 {
        unsafe { gl_create_vertex_array() }
    }
    fn delete_vertex_array(&mut self, array: u32) {
        unsafe { gl_delete_vertex_array(array) }
    }
    fn bind_vertex_array(&mut self, array: u32) {
        unsafe { gl_bind_vertex_array(array) }
    }
    fn enable_vertex_attrib_array(&mut self, index: u32) {
        unsafe { gl_enable_vertex_attrib_array(index) }
    }
    fn vertex_attrib_pointer(&mut self, index: u32, size: i32, kind: u32, normalized: bool, stride: i32, offset: usize) {
        unsafe { gl_vertex_attrib_pointer(index, size, kind, normalized as u32, stride, offset as i32) }
    }
    fn vertex_attrib_i_pointer(&mut self, index: u32, size: i32, kind: u32, stride: i32, offset: usize) {
        unsafe { gl_vertex_attrib_i_pointer(index, size, kind, stride, offset as i32) }
    }
    fn vertex_attrib_divisor(&mut self, index: u32, divisor: u32) {
        unsafe { gl_vertex_attrib_divisor(index, divisor) }
    }

    fn create_texture(&mut self) -> u32 {
        unsafe { gl_create_texture() }
    }
    fn delete_texture(&mut self, texture: u32) {
        unsafe { gl_delete_texture(texture) }
    }
    fn active_texture(&mut self, unit: u32) {
        unsafe { gl_active_texture(unit) }
    }
    fn bind_texture(&mut self, texture: u32) {
        unsafe { gl_bind_texture(GL_TEXTURE_2D, texture) }
    }
    fn tex_parameteri(&mut self, name: u32, value: i32) {
        unsafe { gl_tex_parameteri(GL_TEXTURE_2D, name, value) }
    }
    fn tex_image_2d(&mut self, level: i32, internal: u32, width: i32, height: i32, format: u32, kind: u32, bytes: &[u8]) {
        let pointer = if bytes.is_empty() { std::ptr::null() } else { bytes.as_ptr() };
        unsafe { gl_tex_image_2d(GL_TEXTURE_2D, level, internal as i32, width, height, format, kind, pointer, bytes.len()) }
    }

    fn create_framebuffer(&mut self) -> u32 {
        unsafe { gl_create_framebuffer() }
    }
    fn delete_framebuffer(&mut self, framebuffer: u32) {
        unsafe { gl_delete_framebuffer(framebuffer) }
    }
    fn bind_framebuffer(&mut self, target: u32, framebuffer: u32) {
        unsafe { gl_bind_framebuffer(target, framebuffer) }
    }
    fn framebuffer_texture_2d(&mut self, attachment: u32, texture: u32) {
        unsafe { gl_framebuffer_texture_2d(GL_FRAMEBUFFER, attachment, GL_TEXTURE_2D, texture, 0) }
    }
    fn framebuffer_renderbuffer(&mut self, attachment: u32, renderbuffer: u32) {
        unsafe { gl_framebuffer_renderbuffer(GL_FRAMEBUFFER, attachment, renderbuffer) }
    }
    fn check_framebuffer_status(&mut self) -> u32 {
        unsafe { gl_check_framebuffer_status(GL_FRAMEBUFFER) }
    }
    fn create_renderbuffer(&mut self) -> u32 {
        unsafe { gl_create_renderbuffer() }
    }
    fn delete_renderbuffer(&mut self, renderbuffer: u32) {
        unsafe { gl_delete_renderbuffer(renderbuffer) }
    }
    fn renderbuffer_storage(&mut self, renderbuffer: u32, samples: i32, internal: u32, width: i32, height: i32) {
        unsafe { gl_renderbuffer_storage_multisample(renderbuffer, samples, internal, width, height) }
    }
    fn blit_color(&mut self, from: [i32; 4], to: [i32; 2]) {
        unsafe { gl_blit_framebuffer(from[0], from[1], from[2], from[3], to[0], to[1]) }
    }
    fn scissor(&mut self, rect: [i32; 4]) {
        unsafe { gl_scissor(rect[0], rect[1], rect[2], rect[3]) }
    }
    fn invalidate_framebuffer(&mut self, target: u32, first: u32, second: u32) {
        unsafe { gl_invalidate_framebuffer(target, first, second) }
    }
    fn draw_buffers(&mut self, count: u32) {
        unsafe { gl_draw_buffers(count) }
    }
    fn read_buffer(&mut self, attachment: u32) {
        unsafe { gl_read_buffer(attachment) }
    }
    fn clear_color_buffer(&mut self, index: i32, color: [f32; 4]) {
        unsafe { gl_clear_bufferfv(index, color[0], color[1], color[2], color[3]) }
    }

    fn viewport(&mut self, width: i32, height: i32) {
        unsafe { gl_viewport(0, 0, width, height) }
    }
    fn enable(&mut self, capability: u32) {
        unsafe { gl_enable(capability) }
    }
    fn disable(&mut self, capability: u32) {
        unsafe { gl_disable(capability) }
    }
    fn depth_func(&mut self, function: u32) {
        unsafe { gl_depth_func(function) }
    }
    fn depth_mask(&mut self, write: bool) {
        unsafe { gl_depth_mask(write as u32) }
    }
    fn blend_func_separate(&mut self, source_rgb: u32, destination_rgb: u32, source_alpha: u32, destination_alpha: u32) {
        unsafe { gl_blend_func_separate(source_rgb, destination_rgb, source_alpha, destination_alpha) }
    }
    fn cull_face(&mut self, face: u32) {
        unsafe { gl_cull_face(face) }
    }
    fn front_face(&mut self, winding: u32) {
        unsafe { gl_front_face(winding) }
    }
    fn clear(&mut self, color: [f32; 4], depth: f32) {
        unsafe {
            gl_clear_color(color[0], color[1], color[2], color[3]);
            gl_clear_depth(depth);
            gl_clear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);
        }
    }
    fn draw_arrays(&mut self, mode: u32, first: i32, count: i32) {
        unsafe { gl_draw_arrays(mode, first, count) }
    }
    fn draw_arrays_instanced(&mut self, mode: u32, first: i32, count: i32, instances: i32) {
        unsafe { gl_draw_arrays_instanced(mode, first, count, instances) }
    }
    fn draw_elements_instanced(&mut self, mode: u32, count: i32, kind: u32, offset: usize, instances: i32) {
        unsafe { gl_draw_elements_instanced(mode, count, kind, offset as i32, instances) }
    }
    fn read_pixels(&mut self, width: i32, height: i32, out: &mut [u8]) {
        unsafe { gl_read_pixels(0, 0, width, height, GL_RGBA, GL_UNSIGNED_BYTE, out.as_mut_ptr(), out.len()) }
    }
}

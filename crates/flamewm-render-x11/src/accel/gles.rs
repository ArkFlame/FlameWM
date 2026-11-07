use std::ffi::{CStr, CString, c_char, c_float, c_int, c_uint, c_void};
use std::ptr;

use crate::ffi::dynamic_library::DynamicLibrary;
use flamewm_render_core::{PixelOp, RoundedRect};

mod consts;
use consts::*;
mod loader;
use loader::*;
mod blur_fill;
mod paint;

struct Program {
    id: GlUint,
    texture: GlInt,
    mode: GlInt,
    step: GlInt,
    weights: GlInt,
    taps: GlInt,
    outer: GlInt,
    outer_radius: GlInt,
    hole: GlInt,
    hole_radius: GlInt,
    hole_enabled: GlInt,
    color: GlInt,
    rect: GlInt,
    node: GlInt,
    corner: GlInt,
}

/// Which blend a texture takes in [`GpuBlur::blend_texture`].
enum Blend {
    /// Straight colour (0..1 per channel) through the texture's alpha coverage.
    Mask([f32; 4]),
    /// Premultiplied RGBA8 composited as XRender `PictOpOver`.
    Premultiplied,
}

struct RenderTarget {
    texture: GlUint,
    framebuffer: GlUint,
    width: i32,
    height: i32,
}

pub(crate) struct GpuBlur {
    gl: Gl,
    egl: Egl,
    display: EglDisplay,
    context: EglContext,
    surface: EglSurface,
    copy: Program,
    blur: Program,
    rounded: Program,
    mask: Program,
    image: Program,
    backdrop: Program,
    quad: GlUint,
    /// Reused readback buffer; the caller's frame is replaced only after a successful GL pass.
    readback: Vec<u8>,
    max_texture_size: i32,
    renderer: String,
}

impl GpuBlur {
    pub(crate) fn new() -> Result<Self, String> {
        let egl = Egl::load()?;
        let gl = Gl::load()?;
        let mut last_error = "no EGL display".to_owned();
        for display in egl.displays() {
            // SAFETY: `display` is a non-NULL handle returned by eglGetDisplay/eglGetPlatformDisplayEXT.
            match unsafe { Self::create_on(&egl, display) } {
                Ok((context, surface)) => {
                    // SAFETY: the context created on `display` is current on this thread.
                    return unsafe { Self::finish(egl, gl, display, context, surface) };
                }
                Err(error) => last_error = error,
            }
        }
        Err(last_error)
    }

    unsafe fn create_on(
        egl: &Egl,
        display: EglDisplay,
    ) -> Result<(EglContext, EglSurface), String> {
        let (mut major, mut minor) = (0, 0);
        // SAFETY: `display` is a valid EGL display handle; the out pointers are live locals.
        if unsafe { (egl.initialize)(display, &mut major, &mut minor) } == 0 {
            return Err("eglInitialize failed".to_owned());
        }
        // SAFETY: EGL_OPENGL_ES_API is a valid API enum for eglBindAPI.
        if unsafe { (egl.bind_api)(EGL_OPENGL_ES_API) } == 0 {
            // SAFETY: the display was initialized above.
            unsafe { (egl.terminate)(display) };
            return Err("eglBindAPI(OpenGL ES) failed".to_owned());
        }
        let color = [
            EGL_RED_SIZE,
            8,
            EGL_GREEN_SIZE,
            8,
            EGL_BLUE_SIZE,
            8,
            EGL_ALPHA_SIZE,
            8,
        ];
        let mut config = ptr::null_mut();
        let mut pbuffer = true;
        let mut chosen = false;
        for surface_type in [Some(EGL_PBUFFER_BIT), None] {
            let mut attributes = vec![EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT];
            if let Some(bits) = surface_type {
                attributes.extend([EGL_SURFACE_TYPE, bits]);
            }
            attributes.extend(color);
            attributes.push(EGL_NONE);
            let mut count = 0;
            // SAFETY: the attribute list is EGL_NONE-terminated and room for one config is provided.
            let found = unsafe {
                (egl.choose_config)(display, attributes.as_ptr(), &mut config, 1, &mut count)
            };
            if found != 0 && count > 0 {
                pbuffer = surface_type.is_some();
                chosen = true;
                break;
            }
        }
        if !chosen {
            // SAFETY: the display was initialized above.
            unsafe { (egl.terminate)(display) };
            return Err("no EGL config with OpenGL ES 2 support".to_owned());
        }
        let context_attributes = [EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE];
        // SAFETY: `config` came from eglChooseConfig on this display; the list is EGL_NONE-terminated.
        let context = unsafe {
            (egl.create_context)(
                display,
                config,
                ptr::null_mut(),
                context_attributes.as_ptr(),
            )
        };
        if context.is_null() {
            // SAFETY: the display was initialized above.
            unsafe { (egl.terminate)(display) };
            return Err("eglCreateContext(OpenGL ES 2) failed".to_owned());
        }
        let surface_attributes = [EGL_WIDTH, 1, EGL_HEIGHT, 1, EGL_NONE];
        let surface = if pbuffer {
            // SAFETY: `config` supports pbuffers (requested above); the list is EGL_NONE-terminated.
            unsafe { (egl.create_pbuffer_surface)(display, config, surface_attributes.as_ptr()) }
        } else {
            ptr::null_mut()
        };
        // SAFETY: a NULL surface binds surfaceless, which EGL_KHR_surfaceless_context permits.
        if unsafe { (egl.make_current)(display, surface, surface, context) } == 0 {
            // SAFETY: the context and optional surface belong to this initialized display.
            unsafe {
                if !surface.is_null() {
                    (egl.destroy_surface)(display, surface);
                }
                (egl.destroy_context)(display, context);
                (egl.terminate)(display);
            }
            return Err("eglMakeCurrent failed".to_owned());
        }
        Ok((context, surface))
    }

    unsafe fn finish(
        egl: Egl,
        gl: Gl,
        display: EglDisplay,
        context: EglContext,
        surface: EglSurface,
    ) -> Result<Self, String> {
        // SAFETY: a context is current; GL_RENDERER returns a static NUL-terminated string or NULL.
        let renderer = unsafe {
            let raw = (gl.get_string)(GL_RENDERER);
            if raw.is_null() {
                "unknown OpenGL ES renderer".to_owned()
            } else {
                CStr::from_ptr(raw.cast()).to_string_lossy().into_owned()
            }
        };
        let mut max_texture_size = 0;
        // SAFETY: a context is current and the out pointer is a live local.
        unsafe { (gl.get_integerv)(GL_MAX_TEXTURE_SIZE, &mut max_texture_size) };
        let mut gpu = Self {
            copy: Program::empty(),
            blur: Program::empty(),
            rounded: Program::empty(),
            mask: Program::empty(),
            image: Program::empty(),
            backdrop: Program::empty(),
            readback: Vec::new(),
            quad: 0,
            max_texture_size,
            renderer,
            gl,
            egl,
            display,
            context,
            surface,
        };
        // SAFETY: the context owned by `gpu` is current on this thread.
        unsafe { gpu.build()? };
        Ok(gpu)
    }

    unsafe fn build(&mut self) -> Result<(), String> {
        // SAFETY: the owned context is current; every handle created here is owned by `self`.
        unsafe {
            self.copy = self.program(COPY_SHADER)?;
            self.blur = self.program(BLUR_SHADER)?;
            self.rounded = self.program(ROUNDED_SHADER)?;
            self.mask = self.program(MASK_SHADER)?;
            self.image = self.program(IMAGE_SHADER)?;
            self.backdrop = self.program(BACKDROP_SHADER)?;
            let vertices: [f32; 8] = [-1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
            (self.gl.gen_buffers)(1, &mut self.quad);
            (self.gl.bind_buffer)(GL_ARRAY_BUFFER, self.quad);
            (self.gl.buffer_data)(
                GL_ARRAY_BUFFER,
                std::mem::size_of_val(&vertices) as isize,
                vertices.as_ptr().cast(),
                GL_STATIC_DRAW,
            );
            (self.gl.vertex_attrib_pointer)(POSITION_ATTRIBUTE, 2, GL_FLOAT, 0, 0, ptr::null());
            (self.gl.enable_vertex_attrib_array)(POSITION_ATTRIBUTE);
            (self.gl.disable)(GL_BLEND);
            (self.gl.pixel_storei)(GL_PACK_ALIGNMENT, 1);
            (self.gl.pixel_storei)(GL_UNPACK_ALIGNMENT, 1);
            self.check("pipeline setup")
        }
    }

    pub(crate) fn renderer(&self) -> &str {
        &self.renderer
    }

    unsafe fn shader(&self, kind: GlEnum, source: &str) -> Result<GlUint, String> {
        let source = CString::new(source).expect("shader source is static and NUL-free");
        // SAFETY: a context is current; the source pointer stays alive for the call.
        unsafe {
            let shader = (self.gl.create_shader)(kind);
            let pointer = source.as_ptr();
            (self.gl.shader_source)(shader, 1, &pointer, ptr::null());
            (self.gl.compile_shader)(shader);
            let mut status = 0;
            (self.gl.get_shaderiv)(shader, GL_COMPILE_STATUS, &mut status);
            if status == 0 {
                let mut log = [0 as c_char; 1024];
                let mut length = 0;
                (self.gl.get_shader_info_log)(shader, 1024, &mut length, log.as_mut_ptr());
                (self.gl.delete_shader)(shader);
                return Err(format!(
                    "shader compile failed: {}",
                    CStr::from_ptr(log.as_ptr()).to_string_lossy()
                ));
            }
            Ok(shader)
        }
    }

    unsafe fn program(&self, fragment: &str) -> Result<Program, String> {
        // SAFETY: a context is current; shaders are deleted once linked into the program.
        unsafe {
            let vertex = self.shader(GL_VERTEX_SHADER, VERTEX_SHADER)?;
            let fragment = match self.shader(GL_FRAGMENT_SHADER, fragment) {
                Ok(shader) => shader,
                Err(error) => {
                    (self.gl.delete_shader)(vertex);
                    return Err(error);
                }
            };
            let id = (self.gl.create_program)();
            (self.gl.attach_shader)(id, vertex);
            (self.gl.attach_shader)(id, fragment);
            (self.gl.bind_attrib_location)(id, POSITION_ATTRIBUTE, c"a_position".as_ptr());
            (self.gl.link_program)(id);
            (self.gl.delete_shader)(vertex);
            (self.gl.delete_shader)(fragment);
            let mut status = 0;
            (self.gl.get_programiv)(id, GL_LINK_STATUS, &mut status);
            if status == 0 {
                let mut log = [0 as c_char; 1024];
                let mut length = 0;
                (self.gl.get_program_info_log)(id, 1024, &mut length, log.as_mut_ptr());
                (self.gl.delete_program)(id);
                return Err(format!(
                    "program link failed: {}",
                    CStr::from_ptr(log.as_ptr()).to_string_lossy()
                ));
            }
            let uniform = |name: &CStr| (self.gl.get_uniform_location)(id, name.as_ptr());
            Ok(Program {
                id,
                texture: uniform(c"u_texture"),
                mode: uniform(c"u_mode"),
                step: uniform(c"u_step"),
                weights: uniform(c"u_weights"),
                taps: uniform(c"u_taps"),
                outer: uniform(c"u_outer"),
                outer_radius: uniform(c"u_outer_radius"),
                hole: uniform(c"u_hole"),
                hole_radius: uniform(c"u_hole_radius"),
                hole_enabled: uniform(c"u_hole_enabled"),
                color: uniform(c"u_color"),
                rect: uniform(c"u_rect"),
                node: uniform(c"u_node"),
                corner: uniform(c"u_corner"),
            })
        }
    }

    unsafe fn check(&self, stage: &str) -> Result<(), String> {
        // SAFETY: a context is current.
        let error = unsafe { (self.gl.get_error)() };
        if error == GL_NO_ERROR {
            Ok(())
        } else {
            Err(format!("OpenGL ES error 0x{error:04x} during {stage}"))
        }
    }

    /// A `width` x `height` RGBA8 texture filled from `pixels` (NULL leaves it undefined), with
    /// clamped edges and `filter` sampling. The caller deletes it.
    ///
    /// # Safety
    /// A context must be current; `pixels` is NULL or points to `width * height * 4` bytes.
    unsafe fn upload_texture(
        &self,
        width: i32,
        height: i32,
        pixels: *const c_void,
        filter: GlInt,
    ) -> GlUint {
        // SAFETY: forwarded from this function's contract.
        unsafe {
            let mut texture = 0;
            (self.gl.gen_textures)(1, &mut texture);
            (self.gl.bind_texture)(GL_TEXTURE_2D, texture);
            (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, filter);
            (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, filter);
            (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
            (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
            (self.gl.tex_image_2d)(
                GL_TEXTURE_2D,
                0,
                GL_RGBA as GlInt,
                width,
                height,
                0,
                GL_RGBA,
                GL_UNSIGNED_BYTE,
                pixels,
            );
            texture
        }
    }

    unsafe fn target(
        &self,
        width: i32,
        height: i32,
        pixels: *const c_void,
    ) -> Result<RenderTarget, String> {
        // SAFETY: a context is current; `pixels` is NULL or points to width*height RGBA8 bytes.
        unsafe {
            let texture = self.upload_texture(width, height, pixels, GL_LINEAR);
            let mut framebuffer = 0;
            (self.gl.gen_framebuffers)(1, &mut framebuffer);
            (self.gl.bind_framebuffer)(GL_FRAMEBUFFER, framebuffer);
            (self.gl.framebuffer_texture_2d)(
                GL_FRAMEBUFFER,
                GL_COLOR_ATTACHMENT0,
                GL_TEXTURE_2D,
                texture,
                0,
            );
            let target = RenderTarget {
                texture,
                framebuffer,
                width,
                height,
            };
            if (self.gl.check_framebuffer_status)(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE {
                self.release(&target);
                return Err("RGBA8 framebuffer is incomplete".to_owned());
            }
            Ok(target)
        }
    }

    unsafe fn release(&self, target: &RenderTarget) {
        // SAFETY: a context is current and both handles were created by `target`.
        unsafe {
            (self.gl.delete_framebuffers)(1, &target.framebuffer);
            (self.gl.delete_textures)(1, &target.texture);
        }
    }

    unsafe fn copy_pass(
        &self,
        source: &RenderTarget,
        destination: &RenderTarget,
        mode: i32,
        filter: GlInt,
    ) {
        // SAFETY: a context is current; both targets are live and owned by this pipeline.
        unsafe {
            (self.gl.bind_framebuffer)(GL_FRAMEBUFFER, destination.framebuffer);
            (self.gl.viewport)(0, 0, destination.width, destination.height);
            (self.gl.use_program)(self.copy.id);
            (self.gl.active_texture)(GL_TEXTURE0);
            (self.gl.bind_texture)(GL_TEXTURE_2D, source.texture);
            (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, filter);
            (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, filter);
            (self.gl.uniform1i)(self.copy.texture, 0);
            (self.gl.uniform1i)(self.copy.mode, mode);
            (self.gl.draw_arrays)(GL_TRIANGLE_STRIP, 0, 4);
        }
    }

    unsafe fn blur_pass(
        &self,
        source: &RenderTarget,
        destination: &RenderTarget,
        step: (f32, f32),
        weights: &[f32],
    ) {
        // SAFETY: a context is current; both targets are live and `weights` has at most MAX_TAPS entries.
        unsafe {
            (self.gl.bind_framebuffer)(GL_FRAMEBUFFER, destination.framebuffer);
            (self.gl.viewport)(0, 0, destination.width, destination.height);
            (self.gl.use_program)(self.blur.id);
            (self.gl.active_texture)(GL_TEXTURE0);
            (self.gl.bind_texture)(GL_TEXTURE_2D, source.texture);
            (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
            (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
            (self.gl.uniform1i)(self.blur.texture, 0);
            (self.gl.uniform2f)(self.blur.step, step.0, step.1);
            (self.gl.uniform1fv)(
                self.blur.weights,
                weights.len() as GlSizei,
                weights.as_ptr(),
            );
            (self.gl.uniform1i)(self.blur.taps, weights.len() as GlInt);
            (self.gl.draw_arrays)(GL_TRIANGLE_STRIP, 0, 4);
        }
    }

    /// Checks that `len` bytes are one RGBA8 frame of `width`x`height` that fits the GPU limit.
    fn frame_size(&self, len: usize, width: u32, height: u32) -> Result<(i32, i32), String> {
        let (w, h) = (width as i32, height as i32);
        if w < 1 || h < 1 || w > self.max_texture_size || h > self.max_texture_size {
            return Err(format!(
                "{width}x{height} exceeds the GPU texture limit {}",
                self.max_texture_size
            ));
        }
        if len != width as usize * height as usize * 4 {
            return Err("pixel buffer does not match its size".to_owned());
        }
        Ok((w, h))
    }

    /// Takes the reusable readback buffer, sized for one frame of `len` bytes.
    fn staging(&mut self, len: usize) -> Vec<u8> {
        let mut staged = std::mem::take(&mut self.readback);
        staged.resize(len, 0);
        staged
    }

    /// Reads `target` into `out`, which holds exactly one frame, and reports any GL error.
    unsafe fn read_back(&self, target: &RenderTarget, out: &mut [u8]) -> Result<(), String> {
        // SAFETY: a context is current; `target` is live and `out` holds width*height RGBA8 bytes.
        unsafe {
            (self.gl.bind_framebuffer)(GL_FRAMEBUFFER, target.framebuffer);
            (self.gl.read_pixels)(
                0,
                0,
                target.width,
                target.height,
                GL_RGBA,
                GL_UNSIGNED_BYTE,
                out.as_mut_ptr().cast(),
            );
            (self.gl.bind_framebuffer)(GL_FRAMEBUFFER, 0);
            self.check("readback")
        }
    }
}

impl Program {
    const fn empty() -> Self {
        Self {
            id: 0,
            texture: -1,
            mode: -1,
            step: -1,
            weights: -1,
            taps: -1,
            outer: -1,
            outer_radius: -1,
            hole: -1,
            hole_radius: -1,
            hole_enabled: -1,
            color: -1,
            rect: -1,
            node: -1,
            corner: -1,
        }
    }
}

impl Drop for GpuBlur {
    fn drop(&mut self) {
        // SAFETY: every handle below was created on this display/context, which is made current first.
        unsafe {
            (self.egl.make_current)(self.display, self.surface, self.surface, self.context);
            if self.quad != 0 {
                (self.gl.delete_buffers)(1, &self.quad);
            }
            for program in [
                self.copy.id,
                self.blur.id,
                self.rounded.id,
                self.mask.id,
                self.image.id,
                self.backdrop.id,
            ] {
                if program != 0 {
                    (self.gl.delete_program)(program);
                }
            }
            (self.egl.make_current)(
                self.display,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            );
            if !self.surface.is_null() {
                (self.egl.destroy_surface)(self.display, self.surface);
            }
            (self.egl.destroy_context)(self.display, self.context);
            (self.egl.terminate)(self.display);
        }
    }
}

#[must_use]
pub(crate) fn downscale_factor(sigma: f32) -> i32 {
    if sigma > 8.0 {
        4
    } else if sigma > 4.0 {
        2
    } else {
        1
    }
}

#[must_use]
pub(crate) fn gaussian_weights(sigma: f32) -> Vec<f32> {
    let sigma = sigma.max(0.1);
    let taps = ((sigma * 3.0).ceil() as usize + 1).clamp(1, MAX_TAPS);
    let mut weights: Vec<f32> = (0..taps)
        .map(|offset| (-((offset * offset) as f32) / (2.0 * sigma * sigma)).exp())
        .collect();
    let total = weights[0] + 2.0 * weights[1..].iter().sum::<f32>();
    for weight in &mut weights {
        *weight /= total;
    }
    weights
}

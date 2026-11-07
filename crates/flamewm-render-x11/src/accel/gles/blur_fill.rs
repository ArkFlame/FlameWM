use super::*;

impl GpuBlur {
    /// Source-over `color` through `outer` minus `hole`, computed by the rounded-rect shader. The
    /// caller's frame is replaced only when the GL pass and its readback both succeed.
    pub(crate) fn rounded_fill(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        outer: RoundedRect,
        hole: Option<RoundedRect>,
        color: [u8; 4],
    ) -> Result<(), String> {
        let (w, h) = self.frame_size(pixels.len(), width, height)?;
        if outer.width == 0 || outer.height == 0 {
            return Ok(());
        }
        // SAFETY: the owned context is made current before any GL call; every handle is released below.
        if unsafe {
            (self.egl.make_current)(self.display, self.surface, self.surface, self.context)
        } == 0
        {
            return Err("eglMakeCurrent failed".to_owned());
        }
        let mut staged = self.staging(pixels.len());
        // SAFETY: the context is current and `pixels` holds one w*h RGBA8 frame.
        let result = unsafe {
            self.target(w, h, pixels.as_ptr().cast())
                .and_then(|target| {
                    self.rounded_pass(&target, outer, hole, color);
                    let read = self.read_back(&target, &mut staged);
                    self.release(&target);
                    read
                })
        };
        if result.is_ok() {
            pixels.copy_from_slice(&staged);
        }
        self.readback = staged;
        result
    }

    /// Blends the rounded-rect quad over the frame attached to `target`. Blending is
    /// `rgb = src * a + dst * (1 - a)` and `alpha = a + dst.a * (1 - a)`, as the trait documents.
    pub(super) unsafe fn rounded_pass(
        &self,
        target: &RenderTarget,
        outer: RoundedRect,
        hole: Option<RoundedRect>,
        color: [u8; 4],
    ) {
        let rect = |rect: RoundedRect| {
            [
                rect.x as f32,
                rect.y as f32,
                rect.width as f32,
                rect.height as f32,
            ]
        };
        let [r, g, b, a] = color.map(|channel| f32::from(channel) / 255.0);
        // SAFETY: a context is current; `target` is live and owned by the caller.
        unsafe {
            (self.gl.bind_framebuffer)(GL_FRAMEBUFFER, target.framebuffer);
            (self.gl.viewport)(0, 0, target.width, target.height);
            (self.gl.use_program)(self.rounded.id);
            let [x, y, w, h] = rect(outer);
            (self.gl.uniform4f)(self.rounded.outer, x, y, w, h);
            (self.gl.uniform1f)(self.rounded.outer_radius, outer.radius as f32);
            let [x, y, w, h] = hole.map_or([0.0; 4], rect);
            (self.gl.uniform4f)(self.rounded.hole, x, y, w, h);
            (self.gl.uniform1f)(
                self.rounded.hole_radius,
                hole.map_or(0.0, |hole| hole.radius as f32),
            );
            (self.gl.uniform1i)(self.rounded.hole_enabled, i32::from(hole.is_some()));
            (self.gl.uniform4f)(self.rounded.color, r, g, b, a);
            (self.gl.enable)(GL_BLEND);
            (self.gl.blend_func_separate)(
                GL_SRC_ALPHA,
                GL_ONE_MINUS_SRC_ALPHA,
                GL_ONE,
                GL_ONE_MINUS_SRC_ALPHA,
            );
            (self.gl.draw_arrays)(GL_TRIANGLE_STRIP, 0, 4);
            (self.gl.disable)(GL_BLEND);
        }
    }

    /// Uploads `pixels` (RGBA8 for `rect`'s size) and blends it over `current` at `rect`. The texture
    /// is freed whether or not the GL calls succeed, and `current` changes only through the blend.
    ///
    /// # Safety
    /// The context owned by `self` must be current, and `current` must belong to it.
    pub(super) unsafe fn blend_texture(
        &self,
        current: &RenderTarget,
        rect: (i32, i32, u32, u32),
        pixels: &[u8],
        blend: Blend,
    ) -> Result<(), String> {
        let (x, y, width, height) = rect;
        let (w, h) = (width as i32, height as i32);
        if w > self.max_texture_size || h > self.max_texture_size {
            return Err(format!(
                "{width}x{height} blend exceeds the GPU texture limit {}",
                self.max_texture_size
            ));
        }
        // SAFETY: the caller made the context current; the texture is deleted below.
        unsafe {
            let texture = self.upload_texture(w, h, pixels.as_ptr().cast(), GL_NEAREST);
            let rect = [x as f32, y as f32, width as f32, height as f32];
            let result = self.check("blend texture upload").and_then(|()| {
                match blend {
                    Blend::Mask(color) => {
                        self.blend_pass(current, &self.mask, rect, Some(color), texture, false)
                    }
                    Blend::Premultiplied => {
                        self.blend_pass(current, &self.image, rect, None, texture, true)
                    }
                }
                self.check("blend pass")
            });
            (self.gl.delete_textures)(1, &texture);
            result
        }
    }

    /// Draws a full-frame quad with `program`, which keeps only the texels inside `rect`, and blends
    /// it over `current`. Masks blend straight colour through alpha; premultiplied images use
    /// `ONE, ONE_MINUS_SRC_ALPHA`.
    ///
    /// # Safety
    /// The context owned by `self` must be current, and `current` and `texture` must belong to it.
    pub(super) unsafe fn blend_pass(
        &self,
        current: &RenderTarget,
        program: &Program,
        rect: [f32; 4],
        color: Option<[f32; 4]>,
        texture: GlUint,
        premultiplied: bool,
    ) {
        // SAFETY: a context is current; `current` and `texture` are live and owned by the caller.
        unsafe {
            (self.gl.bind_framebuffer)(GL_FRAMEBUFFER, current.framebuffer);
            (self.gl.viewport)(0, 0, current.width, current.height);
            (self.gl.use_program)(program.id);
            (self.gl.active_texture)(GL_TEXTURE0);
            (self.gl.bind_texture)(GL_TEXTURE_2D, texture);
            (self.gl.uniform1i)(program.texture, 0);
            (self.gl.uniform4f)(program.rect, rect[0], rect[1], rect[2], rect[3]);
            if let Some([r, g, b, a]) = color {
                (self.gl.uniform4f)(program.color, r, g, b, a);
            }
            (self.gl.enable)(GL_BLEND);
            if premultiplied {
                (self.gl.blend_func_separate)(
                    GL_ONE,
                    GL_ONE_MINUS_SRC_ALPHA,
                    GL_ONE,
                    GL_ONE_MINUS_SRC_ALPHA,
                );
            } else {
                (self.gl.blend_func_separate)(
                    GL_SRC_ALPHA,
                    GL_ONE_MINUS_SRC_ALPHA,
                    GL_ONE,
                    GL_ONE_MINUS_SRC_ALPHA,
                );
            }
            (self.gl.draw_arrays)(GL_TRIANGLE_STRIP, 0, 4);
            (self.gl.disable)(GL_BLEND);
        }
    }

    pub(crate) fn blur_sigma(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        sigma: f32,
    ) -> Result<(), String> {
        let (w, h) = self.frame_size(pixels.len(), width, height)?;
        let mut staged = self.staging(pixels.len());
        // SAFETY: the context owned by `self` is made current on this thread before any GL call;
        // every texture and framebuffer created here is released before returning.
        unsafe {
            if (self.egl.make_current)(self.display, self.surface, self.surface, self.context) == 0
            {
                return Err("eglMakeCurrent failed".to_owned());
            }
            let source = self.target(w, h, pixels.as_ptr().cast())?;
            let result = self.blur_target(&source, sigma).and_then(|output| {
                let read = self.read_back(&output, &mut staged);
                self.release(&output);
                read
            });
            self.release(&source);
            (self.gl.bind_framebuffer)(GL_FRAMEBUFFER, 0);
            if result.is_ok() {
                pixels.copy_from_slice(&staged);
            }
            self.readback = staged;
            result
        }
    }
}

use super::*;

impl GpuBlur {
    /// Applies `ops` to one frame: the frame is uploaded once, every op runs on the frame-resident
    /// target, and one readback returns the result. `pixels` changes only when every op succeeds.
    pub(crate) fn paint_ops(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        ops: &[PixelOp<'_>],
    ) -> Result<(), String> {
        let (w, h) = self.frame_size(pixels.len(), width, height)?;
        let mut staged = self.staging(pixels.len());
        // SAFETY: the context owned by `self` is made current before any GL call, and every target
        // created here is released before returning.
        unsafe {
            if (self.egl.make_current)(self.display, self.surface, self.surface, self.context) == 0
            {
                self.readback = staged;
                return Err("eglMakeCurrent failed".to_owned());
            }
            let result = self
                .target(w, h, pixels.as_ptr().cast())
                .and_then(|mut current| {
                    let result = self
                        .apply_ops(&mut current, ops)
                        .and_then(|()| self.read_back(&current, &mut staged));
                    self.release(&current);
                    result
                });
            (self.gl.bind_framebuffer)(GL_FRAMEBUFFER, 0);
            if result.is_ok() {
                pixels.copy_from_slice(&staged);
            }
            self.readback = staged;
            result
        }
    }

    /// Runs `ops` in order on `current`. A blur replaces `current` with its output target.
    ///
    /// # Safety
    /// The context owned by `self` must be current, and `current` must belong to it.
    pub(super) unsafe fn apply_ops(
        &self,
        current: &mut RenderTarget,
        ops: &[PixelOp<'_>],
    ) -> Result<(), String> {
        for op in ops {
            match *op {
                PixelOp::FillRoundedRect { outer, hole, color } => {
                    if outer.width > 0 && outer.height > 0 {
                        // SAFETY: forwarded from this function's contract.
                        unsafe { self.rounded_pass(current, outer, hole, color) };
                    }
                }
                PixelOp::FillRoundedRectClipped {
                    outer,
                    hole,
                    color,
                    clip,
                } => {
                    if outer.width > 0 && outer.height > 0 && clip.2 > 0 && clip.3 > 0 {
                        let inside = i64::from(clip.0) >= 0
                            && i64::from(clip.1) >= 0
                            && i64::from(clip.0) + i64::from(clip.2) <= i64::from(current.width)
                            && i64::from(clip.1) + i64::from(clip.3) <= i64::from(current.height);
                        if !inside {
                            return Err("clipped fill leaves the GPU frame".to_owned());
                        }
                        // SAFETY: forwarded from this function's contract; the scissor is
                        // restored before the next op whatever the pass does.
                        unsafe {
                            (self.gl.enable)(GL_SCISSOR_TEST);
                            (self.gl.scissor)(clip.0, clip.1, clip.2 as GlSizei, clip.3 as GlSizei);
                            self.rounded_pass(current, outer, hole, color);
                            (self.gl.disable)(GL_SCISSOR_TEST);
                        }
                    }
                }
                PixelOp::BlendMask {
                    x,
                    y,
                    width,
                    height,
                    mask,
                    color,
                } => {
                    if mask.len() != width as usize * height as usize {
                        return Err("blend mask does not match its size".to_owned());
                    }
                    if width > 0 && height > 0 {
                        // Coverage expands to RGBA so the mask shares the RGBA texture upload.
                        let expanded: Vec<u8> = mask
                            .iter()
                            .flat_map(|&coverage| [0, 0, 0, coverage])
                            .collect();
                        let color = color.map(|channel| f32::from(channel) / 255.0);
                        // SAFETY: forwarded from this function's contract.
                        unsafe {
                            self.blend_texture(
                                current,
                                (x, y, width, height),
                                &expanded,
                                Blend::Mask(color),
                            )?
                        };
                    }
                }
                PixelOp::BlendImage {
                    x,
                    y,
                    width,
                    height,
                    rgba,
                } => {
                    if rgba.len() != width as usize * height as usize * 4 {
                        return Err("blend image does not match its size".to_owned());
                    }
                    if width > 0 && height > 0 {
                        // SAFETY: forwarded from this function's contract.
                        unsafe {
                            self.blend_texture(
                                current,
                                (x, y, width, height),
                                rgba,
                                Blend::Premultiplied,
                            )?
                        };
                    }
                }
                PixelOp::BackdropBlur {
                    clip,
                    node,
                    corner,
                    radius,
                } => {
                    if clip.2 > 0 && clip.3 > 0 && radius > 0.0 {
                        // SAFETY: forwarded from this function's contract.
                        unsafe { self.backdrop_pass(current, clip, node, corner, radius)? };
                    }
                }
                PixelOp::Blur { radius } => {
                    // SAFETY: forwarded from this function's contract; `current` is replaced only
                    // after its successor exists, and the predecessor is released exactly once.
                    let next = unsafe { self.blur_target(current, radius / 2.0)? };
                    unsafe { self.release(current) };
                    *current = next;
                }
            }
        }
        Ok(())
    }

    /// A node backdrop on the frame-resident target: copies the clip rect out of `current` into a
    /// clip-sized texture, blurs it with the same passes as [`PixelBackend::blur_rgba`] at sigma
    /// `radius / 2`, then writes it opaque inside the node shape. Nothing leaves the GPU.
    ///
    /// # Safety
    /// The context owned by `self` must be current, and `current` must belong to it.
    pub(super) unsafe fn backdrop_pass(
        &self,
        current: &RenderTarget,
        clip: (i32, i32, u32, u32),
        node: (i32, i32, u32, u32),
        corner: u32,
        radius: f32,
    ) -> Result<(), String> {
        let (x, y, width, height) = clip;
        let inside = i64::from(x) >= 0
            && i64::from(y) >= 0
            && i64::from(x) + i64::from(width) <= i64::from(current.width)
            && i64::from(y) + i64::from(height) <= i64::from(current.height);
        if !inside {
            return Err("backdrop clip leaves the GPU frame".to_owned());
        }
        let (cw, ch) = (width as i32, height as i32);
        if cw > self.max_texture_size || ch > self.max_texture_size {
            return Err(format!(
                "{width}x{height} backdrop exceeds the GPU texture limit {}",
                self.max_texture_size
            ));
        }
        // SAFETY: the context is current; every target created here is released before returning.
        unsafe {
            let region = self.target(cw, ch, ptr::null())?;
            (self.gl.bind_framebuffer)(GL_FRAMEBUFFER, current.framebuffer);
            (self.gl.active_texture)(GL_TEXTURE0);
            (self.gl.bind_texture)(GL_TEXTURE_2D, region.texture);
            (self.gl.copy_tex_sub_image_2d)(GL_TEXTURE_2D, 0, 0, 0, x, y, cw, ch);
            let blurred = self
                .check("backdrop copy")
                .and_then(|()| self.blur_target(&region, radius / 2.0));
            self.release(&region);
            let blurred = blurred?;
            let result = self.backdrop_composite(current, &blurred, clip, node, corner);
            self.release(&blurred);
            result
        }
    }

    /// Writes `blurred` opaque over `clip` of `current` where the rounded node shape holds. The
    /// scissor keeps the quad to the clip and the shader discards every other texel, so the frame
    /// outside the shape is untouched. Blending is off, so the blurred colour replaces the pixel.
    ///
    /// # Safety
    /// The context owned by `self` must be current; `current` and `blurred` must belong to it.
    pub(super) unsafe fn backdrop_composite(
        &self,
        current: &RenderTarget,
        blurred: &RenderTarget,
        clip: (i32, i32, u32, u32),
        node: (i32, i32, u32, u32),
        corner: u32,
    ) -> Result<(), String> {
        // SAFETY: a context is current; both targets are live and owned by the caller.
        unsafe {
            (self.gl.bind_framebuffer)(GL_FRAMEBUFFER, current.framebuffer);
            (self.gl.viewport)(0, 0, current.width, current.height);
            (self.gl.enable)(GL_SCISSOR_TEST);
            (self.gl.scissor)(clip.0, clip.1, clip.2 as GlSizei, clip.3 as GlSizei);
            (self.gl.use_program)(self.backdrop.id);
            (self.gl.active_texture)(GL_TEXTURE0);
            (self.gl.bind_texture)(GL_TEXTURE_2D, blurred.texture);
            (self.gl.uniform1i)(self.backdrop.texture, 0);
            (self.gl.uniform4f)(
                self.backdrop.rect,
                clip.0 as f32,
                clip.1 as f32,
                clip.2 as f32,
                clip.3 as f32,
            );
            (self.gl.uniform4f)(
                self.backdrop.node,
                node.0 as f32,
                node.1 as f32,
                node.2 as f32,
                node.3 as f32,
            );
            (self.gl.uniform1f)(self.backdrop.corner, corner as f32);
            (self.gl.draw_arrays)(GL_TRIANGLE_STRIP, 0, 4);
            (self.gl.disable)(GL_SCISSOR_TEST);
            self.check("backdrop composite")
        }
    }

    /// Blurs `source` into a new frame-sized target and returns it to the caller, who releases it.
    /// Intermediate targets are released before returning.
    ///
    /// # Safety
    /// The context owned by `self` must be current, and `source` must belong to it.
    pub(super) unsafe fn blur_target(
        &self,
        source: &RenderTarget,
        sigma: f32,
    ) -> Result<RenderTarget, String> {
        let mut intermediates = Vec::new();
        // SAFETY: forwarded from this function's contract.
        let result = unsafe { self.blur_passes(source, sigma, &mut intermediates) };
        for target in &intermediates {
            // SAFETY: each intermediate was created by `blur_passes` and is released once here.
            unsafe { self.release(target) };
        }
        result
    }

    /// The blur passes behind [`Self::blur_target`]. Every created target is pushed onto
    /// `intermediates` before it can fail, except the returned output.
    ///
    /// # Safety
    /// Same contract as [`Self::blur_target`].
    pub(super) unsafe fn blur_passes(
        &self,
        source: &RenderTarget,
        sigma: f32,
        intermediates: &mut Vec<RenderTarget>,
    ) -> Result<RenderTarget, String> {
        let (w, h) = (source.width, source.height);
        let factor = downscale_factor(sigma);
        let small = ((w + factor - 1) / factor, (h + factor - 1) / factor);
        let weights = gaussian_weights(sigma / factor as f32);
        // SAFETY: the context is current and every target belongs to it, per this function's contract.
        unsafe {
            intermediates.push(self.target(w, h, ptr::null())?);
            let mut current = intermediates.len() - 1;
            self.copy_pass(source, &intermediates[current], 1, GL_NEAREST);
            let mut size = (w, h);
            while size.0 > small.0 || size.1 > small.1 {
                let next = (
                    ((size.0 + 1) / 2).max(small.0),
                    ((size.1 + 1) / 2).max(small.1),
                );
                intermediates.push(self.target(next.0, next.1, ptr::null())?);
                let last = intermediates.len() - 1;
                self.copy_pass(&intermediates[current], &intermediates[last], 0, GL_LINEAR);
                current = last;
                size = next;
            }
            intermediates.push(self.target(size.0, size.1, ptr::null())?);
            let scratch = intermediates.len() - 1;
            self.blur_pass(
                &intermediates[current],
                &intermediates[scratch],
                (1.0 / size.0 as f32, 0.0),
                &weights,
            );
            self.blur_pass(
                &intermediates[scratch],
                &intermediates[current],
                (0.0, 1.0 / size.1 as f32),
                &weights,
            );
            let output = self.target(w, h, ptr::null())?;
            self.copy_pass(&intermediates[current], &output, 2, GL_LINEAR);
            Ok(output)
        }
    }
}

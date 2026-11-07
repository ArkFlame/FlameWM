use super::*;

pub(super) struct Egl {
    pub(super) _library: DynamicLibrary,
    pub(super) get_proc_address: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    pub(super) get_display: unsafe extern "C" fn(*mut c_void) -> EglDisplay,
    pub(super) initialize: unsafe extern "C" fn(EglDisplay, *mut EglInt, *mut EglInt) -> EglBoolean,
    pub(super) terminate: unsafe extern "C" fn(EglDisplay) -> EglBoolean,
    pub(super) query_string: unsafe extern "C" fn(EglDisplay, EglInt) -> *const c_char,
    pub(super) bind_api: unsafe extern "C" fn(c_uint) -> EglBoolean,
    pub(super) choose_config: unsafe extern "C" fn(
        EglDisplay,
        *const EglInt,
        *mut EglConfig,
        EglInt,
        *mut EglInt,
    ) -> EglBoolean,
    pub(super) create_context:
        unsafe extern "C" fn(EglDisplay, EglConfig, EglContext, *const EglInt) -> EglContext,
    pub(super) destroy_context: unsafe extern "C" fn(EglDisplay, EglContext) -> EglBoolean,
    pub(super) create_pbuffer_surface:
        unsafe extern "C" fn(EglDisplay, EglConfig, *const EglInt) -> EglSurface,
    pub(super) destroy_surface: unsafe extern "C" fn(EglDisplay, EglSurface) -> EglBoolean,
    pub(super) make_current:
        unsafe extern "C" fn(EglDisplay, EglSurface, EglSurface, EglContext) -> EglBoolean,
}

impl Egl {
    pub(super) fn load() -> Result<Self, String> {
        let library = DynamicLibrary::open(&["libEGL.so.1", "libEGL.so"])?;
        // SAFETY: each symbol is resolved from libEGL with the EGL 1.4 C signature it is cast to.
        unsafe {
            Ok(Self {
                get_proc_address: library.symbol(b"eglGetProcAddress\0")?,
                get_display: library.symbol(b"eglGetDisplay\0")?,
                initialize: library.symbol(b"eglInitialize\0")?,
                terminate: library.symbol(b"eglTerminate\0")?,
                query_string: library.symbol(b"eglQueryString\0")?,
                bind_api: library.symbol(b"eglBindAPI\0")?,
                choose_config: library.symbol(b"eglChooseConfig\0")?,
                create_context: library.symbol(b"eglCreateContext\0")?,
                destroy_context: library.symbol(b"eglDestroyContext\0")?,
                create_pbuffer_surface: library.symbol(b"eglCreatePbufferSurface\0")?,
                destroy_surface: library.symbol(b"eglDestroySurface\0")?,
                make_current: library.symbol(b"eglMakeCurrent\0")?,
                _library: library,
            })
        }
    }

    pub(super) fn extension_proc(&self, name: &str) -> *mut c_void {
        let name = CString::new(name).expect("extension name is static and NUL-free");
        // SAFETY: eglGetProcAddress accepts any NUL-terminated name and returns NULL when unknown.
        unsafe { (self.get_proc_address)(name.as_ptr()) }
    }

    pub(super) fn client_extensions(&self) -> String {
        // SAFETY: EGL_NO_DISPLAY with EGL_EXTENSIONS queries client extensions; NULL means none.
        let raw = unsafe { (self.query_string)(ptr::null_mut(), EGL_EXTENSIONS) };
        if raw.is_null() {
            return String::new();
        }
        // SAFETY: a non-NULL EGL string is NUL-terminated and owned by the implementation.
        unsafe { CStr::from_ptr(raw) }
            .to_string_lossy()
            .into_owned()
    }

    pub(super) fn displays(&self) -> Vec<EglDisplay> {
        let mut displays = Vec::new();
        let extensions = self.client_extensions();
        let platform_display = self.extension_proc("eglGetPlatformDisplayEXT");
        if !platform_display.is_null() {
            // SAFETY: eglGetPlatformDisplayEXT has this signature per EGL_EXT_platform_base.
            let platform_display: unsafe extern "C" fn(
                c_uint,
                *mut c_void,
                *const EglInt,
            ) -> EglDisplay = unsafe { std::mem::transmute(platform_display) };
            if extensions.contains("EGL_MESA_platform_surfaceless") {
                // SAFETY: the surfaceless platform takes EGL_DEFAULT_DISPLAY and no attributes.
                let display = unsafe {
                    platform_display(EGL_PLATFORM_SURFACELESS_MESA, ptr::null_mut(), ptr::null())
                };
                displays.push(display);
            }
            let query_devices = self.extension_proc("eglQueryDevicesEXT");
            if extensions.contains("EGL_EXT_platform_device") && !query_devices.is_null() {
                // SAFETY: eglQueryDevicesEXT has this signature per EGL_EXT_device_enumeration.
                let query_devices: unsafe extern "C" fn(
                    EglInt,
                    *mut EglDevice,
                    *mut EglInt,
                ) -> EglBoolean = unsafe { std::mem::transmute(query_devices) };
                let mut devices = [ptr::null_mut(); 4];
                let mut count = 0;
                // SAFETY: the output array holds 4 devices and `count` receives how many were written.
                if unsafe { query_devices(4, devices.as_mut_ptr(), &mut count) } != 0 {
                    for device in devices.iter().take(count.clamp(0, 4) as usize) {
                        // SAFETY: each device handle came from eglQueryDevicesEXT just above.
                        let display = unsafe {
                            platform_display(EGL_PLATFORM_DEVICE_EXT, *device, ptr::null())
                        };
                        displays.push(display);
                    }
                }
            }
        }
        // SAFETY: EGL_DEFAULT_DISPLAY (NULL) selects the implementation's default platform.
        displays.push(unsafe { (self.get_display)(ptr::null_mut()) });
        displays.retain(|display| !display.is_null());
        displays
    }
}

pub(super) struct Gl {
    pub(super) _library: DynamicLibrary,
    pub(super) gen_textures: unsafe extern "C" fn(GlSizei, *mut GlUint),
    pub(super) delete_textures: unsafe extern "C" fn(GlSizei, *const GlUint),
    pub(super) bind_texture: unsafe extern "C" fn(GlEnum, GlUint),
    pub(super) tex_image_2d: unsafe extern "C" fn(
        GlEnum,
        GlInt,
        GlInt,
        GlSizei,
        GlSizei,
        GlInt,
        GlEnum,
        GlEnum,
        *const c_void,
    ),
    pub(super) tex_parameteri: unsafe extern "C" fn(GlEnum, GlEnum, GlInt),
    pub(super) active_texture: unsafe extern "C" fn(GlEnum),
    pub(super) gen_framebuffers: unsafe extern "C" fn(GlSizei, *mut GlUint),
    pub(super) delete_framebuffers: unsafe extern "C" fn(GlSizei, *const GlUint),
    pub(super) bind_framebuffer: unsafe extern "C" fn(GlEnum, GlUint),
    pub(super) framebuffer_texture_2d: unsafe extern "C" fn(GlEnum, GlEnum, GlEnum, GlUint, GlInt),
    pub(super) check_framebuffer_status: unsafe extern "C" fn(GlEnum) -> GlEnum,
    pub(super) create_shader: unsafe extern "C" fn(GlEnum) -> GlUint,
    pub(super) shader_source:
        unsafe extern "C" fn(GlUint, GlSizei, *const *const c_char, *const GlInt),
    pub(super) compile_shader: unsafe extern "C" fn(GlUint),
    pub(super) get_shaderiv: unsafe extern "C" fn(GlUint, GlEnum, *mut GlInt),
    pub(super) get_shader_info_log:
        unsafe extern "C" fn(GlUint, GlSizei, *mut GlSizei, *mut c_char),
    pub(super) delete_shader: unsafe extern "C" fn(GlUint),
    pub(super) create_program: unsafe extern "C" fn() -> GlUint,
    pub(super) attach_shader: unsafe extern "C" fn(GlUint, GlUint),
    pub(super) bind_attrib_location: unsafe extern "C" fn(GlUint, GlUint, *const c_char),
    pub(super) link_program: unsafe extern "C" fn(GlUint),
    pub(super) get_programiv: unsafe extern "C" fn(GlUint, GlEnum, *mut GlInt),
    pub(super) get_program_info_log:
        unsafe extern "C" fn(GlUint, GlSizei, *mut GlSizei, *mut c_char),
    pub(super) delete_program: unsafe extern "C" fn(GlUint),
    pub(super) use_program: unsafe extern "C" fn(GlUint),
    pub(super) get_uniform_location: unsafe extern "C" fn(GlUint, *const c_char) -> GlInt,
    pub(super) uniform1i: unsafe extern "C" fn(GlInt, GlInt),
    pub(super) uniform2f: unsafe extern "C" fn(GlInt, c_float, c_float),
    pub(super) uniform1fv: unsafe extern "C" fn(GlInt, GlSizei, *const c_float),
    pub(super) uniform1f: unsafe extern "C" fn(GlInt, c_float),
    pub(super) uniform4f: unsafe extern "C" fn(GlInt, c_float, c_float, c_float, c_float),
    pub(super) enable: unsafe extern "C" fn(GlEnum),
    pub(super) blend_func_separate: unsafe extern "C" fn(GlEnum, GlEnum, GlEnum, GlEnum),
    pub(super) gen_buffers: unsafe extern "C" fn(GlSizei, *mut GlUint),
    pub(super) delete_buffers: unsafe extern "C" fn(GlSizei, *const GlUint),
    pub(super) bind_buffer: unsafe extern "C" fn(GlEnum, GlUint),
    pub(super) buffer_data: unsafe extern "C" fn(GlEnum, isize, *const c_void, GlEnum),
    pub(super) vertex_attrib_pointer:
        unsafe extern "C" fn(GlUint, GlInt, GlEnum, u8, GlSizei, *const c_void),
    pub(super) enable_vertex_attrib_array: unsafe extern "C" fn(GlUint),
    pub(super) viewport: unsafe extern "C" fn(GlInt, GlInt, GlSizei, GlSizei),
    pub(super) draw_arrays: unsafe extern "C" fn(GlEnum, GlInt, GlSizei),
    pub(super) read_pixels:
        unsafe extern "C" fn(GlInt, GlInt, GlSizei, GlSizei, GlEnum, GlEnum, *mut c_void),
    pub(super) pixel_storei: unsafe extern "C" fn(GlEnum, GlInt),
    pub(super) get_integerv: unsafe extern "C" fn(GlEnum, *mut GlInt),
    pub(super) get_string: unsafe extern "C" fn(GlEnum) -> *const u8,
    pub(super) get_error: unsafe extern "C" fn() -> GlEnum,
    pub(super) disable: unsafe extern "C" fn(GlEnum),
    pub(super) copy_tex_sub_image_2d:
        unsafe extern "C" fn(GlEnum, GlInt, GlInt, GlInt, GlInt, GlInt, GlSizei, GlSizei),
    pub(super) scissor: unsafe extern "C" fn(GlInt, GlInt, GlSizei, GlSizei),
}

impl Gl {
    pub(super) fn load() -> Result<Self, String> {
        let library = DynamicLibrary::open(&["libGLESv2.so.2", "libGLESv2.so"])?;
        // SAFETY: each symbol is resolved from libGLESv2 with the OpenGL ES 2.0 C signature it is cast to.
        unsafe {
            Ok(Self {
                gen_textures: library.symbol(b"glGenTextures\0")?,
                delete_textures: library.symbol(b"glDeleteTextures\0")?,
                bind_texture: library.symbol(b"glBindTexture\0")?,
                tex_image_2d: library.symbol(b"glTexImage2D\0")?,
                tex_parameteri: library.symbol(b"glTexParameteri\0")?,
                active_texture: library.symbol(b"glActiveTexture\0")?,
                gen_framebuffers: library.symbol(b"glGenFramebuffers\0")?,
                delete_framebuffers: library.symbol(b"glDeleteFramebuffers\0")?,
                bind_framebuffer: library.symbol(b"glBindFramebuffer\0")?,
                framebuffer_texture_2d: library.symbol(b"glFramebufferTexture2D\0")?,
                check_framebuffer_status: library.symbol(b"glCheckFramebufferStatus\0")?,
                create_shader: library.symbol(b"glCreateShader\0")?,
                shader_source: library.symbol(b"glShaderSource\0")?,
                compile_shader: library.symbol(b"glCompileShader\0")?,
                get_shaderiv: library.symbol(b"glGetShaderiv\0")?,
                get_shader_info_log: library.symbol(b"glGetShaderInfoLog\0")?,
                delete_shader: library.symbol(b"glDeleteShader\0")?,
                create_program: library.symbol(b"glCreateProgram\0")?,
                attach_shader: library.symbol(b"glAttachShader\0")?,
                bind_attrib_location: library.symbol(b"glBindAttribLocation\0")?,
                link_program: library.symbol(b"glLinkProgram\0")?,
                get_programiv: library.symbol(b"glGetProgramiv\0")?,
                get_program_info_log: library.symbol(b"glGetProgramInfoLog\0")?,
                delete_program: library.symbol(b"glDeleteProgram\0")?,
                use_program: library.symbol(b"glUseProgram\0")?,
                get_uniform_location: library.symbol(b"glGetUniformLocation\0")?,
                uniform1i: library.symbol(b"glUniform1i\0")?,
                uniform2f: library.symbol(b"glUniform2f\0")?,
                uniform1fv: library.symbol(b"glUniform1fv\0")?,
                uniform1f: library.symbol(b"glUniform1f\0")?,
                uniform4f: library.symbol(b"glUniform4f\0")?,
                enable: library.symbol(b"glEnable\0")?,
                blend_func_separate: library.symbol(b"glBlendFuncSeparate\0")?,
                gen_buffers: library.symbol(b"glGenBuffers\0")?,
                delete_buffers: library.symbol(b"glDeleteBuffers\0")?,
                bind_buffer: library.symbol(b"glBindBuffer\0")?,
                buffer_data: library.symbol(b"glBufferData\0")?,
                vertex_attrib_pointer: library.symbol(b"glVertexAttribPointer\0")?,
                enable_vertex_attrib_array: library.symbol(b"glEnableVertexAttribArray\0")?,
                viewport: library.symbol(b"glViewport\0")?,
                draw_arrays: library.symbol(b"glDrawArrays\0")?,
                read_pixels: library.symbol(b"glReadPixels\0")?,
                pixel_storei: library.symbol(b"glPixelStorei\0")?,
                get_integerv: library.symbol(b"glGetIntegerv\0")?,
                get_string: library.symbol(b"glGetString\0")?,
                get_error: library.symbol(b"glGetError\0")?,
                disable: library.symbol(b"glDisable\0")?,
                copy_tex_sub_image_2d: library.symbol(b"glCopyTexSubImage2D\0")?,
                scissor: library.symbol(b"glScissor\0")?,
                _library: library,
            })
        }
    }
}

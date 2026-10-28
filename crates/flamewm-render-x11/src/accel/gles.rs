use std::ffi::{CStr, CString, c_char, c_float, c_int, c_uint, c_void};
use std::ptr;

use crate::ffi::dynamic_library::DynamicLibrary;
use flamewm_render_core::{PixelOp, RoundedRect};

type EglDisplay = *mut c_void;
type EglConfig = *mut c_void;
type EglContext = *mut c_void;
type EglSurface = *mut c_void;
type EglDevice = *mut c_void;
type EglBoolean = c_uint;
type EglInt = i32;
type GlUint = c_uint;
type GlInt = c_int;
type GlEnum = c_uint;
type GlSizei = c_int;

const EGL_NONE: EglInt = 0x3038;
const EGL_RENDERABLE_TYPE: EglInt = 0x3040;
const EGL_OPENGL_ES2_BIT: EglInt = 0x0004;
const EGL_SURFACE_TYPE: EglInt = 0x3033;
const EGL_PBUFFER_BIT: EglInt = 0x0001;
const EGL_RED_SIZE: EglInt = 0x3024;
const EGL_GREEN_SIZE: EglInt = 0x3023;
const EGL_BLUE_SIZE: EglInt = 0x3022;
const EGL_ALPHA_SIZE: EglInt = 0x3021;
const EGL_WIDTH: EglInt = 0x3057;
const EGL_HEIGHT: EglInt = 0x3056;
const EGL_CONTEXT_CLIENT_VERSION: EglInt = 0x3098;
const EGL_OPENGL_ES_API: c_uint = 0x30A0;
const EGL_EXTENSIONS: EglInt = 0x3055;
const EGL_PLATFORM_SURFACELESS_MESA: c_uint = 0x31DD;
const EGL_PLATFORM_DEVICE_EXT: c_uint = 0x313F;

const GL_TEXTURE_2D: GlEnum = 0x0DE1;
const GL_RGBA: GlEnum = 0x1908;
const GL_UNSIGNED_BYTE: GlEnum = 0x1401;
const GL_TEXTURE_MIN_FILTER: GlEnum = 0x2801;
const GL_TEXTURE_MAG_FILTER: GlEnum = 0x2800;
const GL_TEXTURE_WRAP_S: GlEnum = 0x2802;
const GL_TEXTURE_WRAP_T: GlEnum = 0x2803;
const GL_LINEAR: GlInt = 0x2601;
const GL_NEAREST: GlInt = 0x2600;
const GL_CLAMP_TO_EDGE: GlInt = 0x812F;
const GL_FRAMEBUFFER: GlEnum = 0x8D40;
const GL_COLOR_ATTACHMENT0: GlEnum = 0x8CE0;
const GL_FRAMEBUFFER_COMPLETE: GlEnum = 0x8CD5;
const GL_VERTEX_SHADER: GlEnum = 0x8B31;
const GL_FRAGMENT_SHADER: GlEnum = 0x8B30;
const GL_COMPILE_STATUS: GlEnum = 0x8B81;
const GL_LINK_STATUS: GlEnum = 0x8B82;
const GL_ARRAY_BUFFER: GlEnum = 0x8892;
const GL_STATIC_DRAW: GlEnum = 0x88E4;
const GL_FLOAT: GlEnum = 0x1406;
const GL_TRIANGLE_STRIP: GlEnum = 0x0005;
const GL_PACK_ALIGNMENT: GlEnum = 0x0D05;
const GL_UNPACK_ALIGNMENT: GlEnum = 0x0CF5;
const GL_MAX_TEXTURE_SIZE: GlEnum = 0x0D33;
const GL_RENDERER: GlEnum = 0x1F01;
const GL_TEXTURE0: GlEnum = 0x84C0;
const GL_BLEND: GlEnum = 0x0BE2;
const GL_ONE: GlEnum = 1;
const GL_SRC_ALPHA: GlEnum = 0x0302;
const GL_ONE_MINUS_SRC_ALPHA: GlEnum = 0x0303;
const GL_NO_ERROR: GlEnum = 0;

const MAX_TAPS: usize = 33;
/// Hard-edged rounded-rect coverage, matching the CPU row insets: a fragment is inside when its
/// pixel centre lies within the signed distance of the rounded rectangle.
const ROUNDED_SHADER: &str = "#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
uniform vec4 u_outer;
uniform float u_outer_radius;
uniform vec4 u_hole;
uniform float u_hole_radius;
uniform int u_hole_enabled;
uniform vec4 u_color;
float rounded_cover(vec4 rect, float radius, vec2 p) {
    vec2 center = rect.xy + rect.zw * 0.5;
    vec2 half_extent = rect.zw * 0.5 - vec2(radius);
    vec2 d = abs(p - center) - half_extent;
    float signed_dist = length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - radius;
    // Coverage ramps across the one-pixel band around the edge, matching the XRender AA edge.
    return clamp(0.5 - signed_dist, 0.0, 1.0);
}
void main() {
    vec2 p = gl_FragCoord.xy;
    float cover = rounded_cover(u_outer, u_outer_radius, p);
    if (u_hole_enabled == 1) {
        cover *= 1.0 - rounded_cover(u_hole, u_hole_radius, p);
    }
    gl_FragColor = vec4(u_color.rgb, u_color.a * cover);
}
";

/// Tinted coverage mask: the texture's alpha is the coverage, `u_rect` places it in frame pixels
/// (row 0 at the bottom of the GL viewport, as the rounded shader), and texels outside it are
/// transparent so the blend leaves the frame unchanged.
const MASK_SHADER: &str = "#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
uniform sampler2D u_texture;
uniform vec4 u_rect;
uniform vec4 u_color;
void main() {
    vec2 uv = (gl_FragCoord.xy - u_rect.xy) / u_rect.zw;
    if (uv.x < 0.0 || uv.y < 0.0 || uv.x >= 1.0 || uv.y >= 1.0) {
        gl_FragColor = vec4(0.0);
        return;
    }
    gl_FragColor = vec4(u_color.rgb, u_color.a * texture2D(u_texture, uv).a);
}
";

/// Premultiplied RGBA image placed by `u_rect`; blended with `ONE, ONE_MINUS_SRC_ALPHA`.
const IMAGE_SHADER: &str = "#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
uniform sampler2D u_texture;
uniform vec4 u_rect;
void main() {
    vec2 uv = (gl_FragCoord.xy - u_rect.xy) / u_rect.zw;
    if (uv.x < 0.0 || uv.y < 0.0 || uv.x >= 1.0 || uv.y >= 1.0) {
        gl_FragColor = vec4(0.0);
        return;
    }
    gl_FragColor = texture2D(u_texture, uv);
}
";

const POSITION_ATTRIBUTE: GlUint = 0;

const VERTEX_SHADER: &str = "attribute vec2 a_position;
varying vec2 v_uv;
void main() {
    v_uv = a_position * 0.5 + 0.5;
    gl_Position = vec4(a_position, 0.0, 1.0);
}
";

const COPY_SHADER: &str = "#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
uniform sampler2D u_texture;
uniform int u_mode;
varying vec2 v_uv;
void main() {
    vec4 color = texture2D(u_texture, v_uv);
    if (u_mode == 1) {
        color = vec4(color.rgb * color.a, color.a);
    } else if (u_mode == 2) {
        color = color.a > 0.0 ? vec4(color.rgb / color.a, color.a) : vec4(0.0);
    }
    gl_FragColor = color;
}
";

const BLUR_SHADER: &str = "#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
uniform sampler2D u_texture;
uniform vec2 u_step;
uniform float u_weights[33];
uniform int u_taps;
varying vec2 v_uv;
void main() {
    vec4 sum = texture2D(u_texture, v_uv) * u_weights[0];
    for (int i = 1; i < 33; i++) {
        if (i >= u_taps) {
            break;
        }
        vec2 offset = u_step * float(i);
        sum += (texture2D(u_texture, v_uv + offset) + texture2D(u_texture, v_uv - offset)) * u_weights[i];
    }
    gl_FragColor = sum;
}
";

struct Egl {
    _library: DynamicLibrary,
    get_proc_address: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    get_display: unsafe extern "C" fn(*mut c_void) -> EglDisplay,
    initialize: unsafe extern "C" fn(EglDisplay, *mut EglInt, *mut EglInt) -> EglBoolean,
    terminate: unsafe extern "C" fn(EglDisplay) -> EglBoolean,
    query_string: unsafe extern "C" fn(EglDisplay, EglInt) -> *const c_char,
    bind_api: unsafe extern "C" fn(c_uint) -> EglBoolean,
    choose_config: unsafe extern "C" fn(
        EglDisplay,
        *const EglInt,
        *mut EglConfig,
        EglInt,
        *mut EglInt,
    ) -> EglBoolean,
    create_context:
        unsafe extern "C" fn(EglDisplay, EglConfig, EglContext, *const EglInt) -> EglContext,
    destroy_context: unsafe extern "C" fn(EglDisplay, EglContext) -> EglBoolean,
    create_pbuffer_surface:
        unsafe extern "C" fn(EglDisplay, EglConfig, *const EglInt) -> EglSurface,
    destroy_surface: unsafe extern "C" fn(EglDisplay, EglSurface) -> EglBoolean,
    make_current:
        unsafe extern "C" fn(EglDisplay, EglSurface, EglSurface, EglContext) -> EglBoolean,
}

impl Egl {
    fn load() -> Result<Self, String> {
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

    fn extension_proc(&self, name: &str) -> *mut c_void {
        let name = CString::new(name).expect("extension name is static and NUL-free");
        // SAFETY: eglGetProcAddress accepts any NUL-terminated name and returns NULL when unknown.
        unsafe { (self.get_proc_address)(name.as_ptr()) }
    }

    fn client_extensions(&self) -> String {
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

    fn displays(&self) -> Vec<EglDisplay> {
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

struct Gl {
    _library: DynamicLibrary,
    gen_textures: unsafe extern "C" fn(GlSizei, *mut GlUint),
    delete_textures: unsafe extern "C" fn(GlSizei, *const GlUint),
    bind_texture: unsafe extern "C" fn(GlEnum, GlUint),
    tex_image_2d: unsafe extern "C" fn(
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
    tex_parameteri: unsafe extern "C" fn(GlEnum, GlEnum, GlInt),
    active_texture: unsafe extern "C" fn(GlEnum),
    gen_framebuffers: unsafe extern "C" fn(GlSizei, *mut GlUint),
    delete_framebuffers: unsafe extern "C" fn(GlSizei, *const GlUint),
    bind_framebuffer: unsafe extern "C" fn(GlEnum, GlUint),
    framebuffer_texture_2d: unsafe extern "C" fn(GlEnum, GlEnum, GlEnum, GlUint, GlInt),
    check_framebuffer_status: unsafe extern "C" fn(GlEnum) -> GlEnum,
    create_shader: unsafe extern "C" fn(GlEnum) -> GlUint,
    shader_source: unsafe extern "C" fn(GlUint, GlSizei, *const *const c_char, *const GlInt),
    compile_shader: unsafe extern "C" fn(GlUint),
    get_shaderiv: unsafe extern "C" fn(GlUint, GlEnum, *mut GlInt),
    get_shader_info_log: unsafe extern "C" fn(GlUint, GlSizei, *mut GlSizei, *mut c_char),
    delete_shader: unsafe extern "C" fn(GlUint),
    create_program: unsafe extern "C" fn() -> GlUint,
    attach_shader: unsafe extern "C" fn(GlUint, GlUint),
    bind_attrib_location: unsafe extern "C" fn(GlUint, GlUint, *const c_char),
    link_program: unsafe extern "C" fn(GlUint),
    get_programiv: unsafe extern "C" fn(GlUint, GlEnum, *mut GlInt),
    get_program_info_log: unsafe extern "C" fn(GlUint, GlSizei, *mut GlSizei, *mut c_char),
    delete_program: unsafe extern "C" fn(GlUint),
    use_program: unsafe extern "C" fn(GlUint),
    get_uniform_location: unsafe extern "C" fn(GlUint, *const c_char) -> GlInt,
    uniform1i: unsafe extern "C" fn(GlInt, GlInt),
    uniform2f: unsafe extern "C" fn(GlInt, c_float, c_float),
    uniform1fv: unsafe extern "C" fn(GlInt, GlSizei, *const c_float),
    uniform1f: unsafe extern "C" fn(GlInt, c_float),
    uniform4f: unsafe extern "C" fn(GlInt, c_float, c_float, c_float, c_float),
    enable: unsafe extern "C" fn(GlEnum),
    blend_func_separate: unsafe extern "C" fn(GlEnum, GlEnum, GlEnum, GlEnum),
    gen_buffers: unsafe extern "C" fn(GlSizei, *mut GlUint),
    delete_buffers: unsafe extern "C" fn(GlSizei, *const GlUint),
    bind_buffer: unsafe extern "C" fn(GlEnum, GlUint),
    buffer_data: unsafe extern "C" fn(GlEnum, isize, *const c_void, GlEnum),
    vertex_attrib_pointer: unsafe extern "C" fn(GlUint, GlInt, GlEnum, u8, GlSizei, *const c_void),
    enable_vertex_attrib_array: unsafe extern "C" fn(GlUint),
    viewport: unsafe extern "C" fn(GlInt, GlInt, GlSizei, GlSizei),
    draw_arrays: unsafe extern "C" fn(GlEnum, GlInt, GlSizei),
    read_pixels: unsafe extern "C" fn(GlInt, GlInt, GlSizei, GlSizei, GlEnum, GlEnum, *mut c_void),
    pixel_storei: unsafe extern "C" fn(GlEnum, GlInt),
    get_integerv: unsafe extern "C" fn(GlEnum, *mut GlInt),
    get_string: unsafe extern "C" fn(GlEnum) -> *const u8,
    get_error: unsafe extern "C" fn() -> GlEnum,
    disable: unsafe extern "C" fn(GlEnum),
}

impl Gl {
    fn load() -> Result<Self, String> {
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
                _library: library,
            })
        }
    }
}

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
    unsafe fn rounded_pass(
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
    unsafe fn blend_texture(
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
    unsafe fn blend_pass(
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
    unsafe fn apply_ops(
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

    /// Blurs `source` into a new frame-sized target and returns it to the caller, who releases it.
    /// Intermediate targets are released before returning.
    ///
    /// # Safety
    /// The context owned by `self` must be current, and `source` must belong to it.
    unsafe fn blur_target(
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
    unsafe fn blur_passes(
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

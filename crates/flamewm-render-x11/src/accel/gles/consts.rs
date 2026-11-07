use super::*;

pub(super) type EglDisplay = *mut c_void;
pub(super) type EglConfig = *mut c_void;
pub(super) type EglContext = *mut c_void;
pub(super) type EglSurface = *mut c_void;
pub(super) type EglDevice = *mut c_void;
pub(super) type EglBoolean = c_uint;
pub(super) type EglInt = i32;
pub(super) type GlUint = c_uint;
pub(super) type GlInt = c_int;
pub(super) type GlEnum = c_uint;
pub(super) type GlSizei = c_int;

pub(super) const EGL_NONE: EglInt = 0x3038;
pub(super) const EGL_RENDERABLE_TYPE: EglInt = 0x3040;
pub(super) const EGL_OPENGL_ES2_BIT: EglInt = 0x0004;
pub(super) const EGL_SURFACE_TYPE: EglInt = 0x3033;
pub(super) const EGL_PBUFFER_BIT: EglInt = 0x0001;
pub(super) const EGL_RED_SIZE: EglInt = 0x3024;
pub(super) const EGL_GREEN_SIZE: EglInt = 0x3023;
pub(super) const EGL_BLUE_SIZE: EglInt = 0x3022;
pub(super) const EGL_ALPHA_SIZE: EglInt = 0x3021;
pub(super) const EGL_WIDTH: EglInt = 0x3057;
pub(super) const EGL_HEIGHT: EglInt = 0x3056;
pub(super) const EGL_CONTEXT_CLIENT_VERSION: EglInt = 0x3098;
pub(super) const EGL_OPENGL_ES_API: c_uint = 0x30A0;
pub(super) const EGL_EXTENSIONS: EglInt = 0x3055;
pub(super) const EGL_PLATFORM_SURFACELESS_MESA: c_uint = 0x31DD;
pub(super) const EGL_PLATFORM_DEVICE_EXT: c_uint = 0x313F;

pub(super) const GL_TEXTURE_2D: GlEnum = 0x0DE1;
pub(super) const GL_RGBA: GlEnum = 0x1908;
pub(super) const GL_UNSIGNED_BYTE: GlEnum = 0x1401;
pub(super) const GL_TEXTURE_MIN_FILTER: GlEnum = 0x2801;
pub(super) const GL_TEXTURE_MAG_FILTER: GlEnum = 0x2800;
pub(super) const GL_TEXTURE_WRAP_S: GlEnum = 0x2802;
pub(super) const GL_TEXTURE_WRAP_T: GlEnum = 0x2803;
pub(super) const GL_LINEAR: GlInt = 0x2601;
pub(super) const GL_NEAREST: GlInt = 0x2600;
pub(super) const GL_CLAMP_TO_EDGE: GlInt = 0x812F;
pub(super) const GL_FRAMEBUFFER: GlEnum = 0x8D40;
pub(super) const GL_COLOR_ATTACHMENT0: GlEnum = 0x8CE0;
pub(super) const GL_FRAMEBUFFER_COMPLETE: GlEnum = 0x8CD5;
pub(super) const GL_VERTEX_SHADER: GlEnum = 0x8B31;
pub(super) const GL_FRAGMENT_SHADER: GlEnum = 0x8B30;
pub(super) const GL_COMPILE_STATUS: GlEnum = 0x8B81;
pub(super) const GL_LINK_STATUS: GlEnum = 0x8B82;
pub(super) const GL_ARRAY_BUFFER: GlEnum = 0x8892;
pub(super) const GL_STATIC_DRAW: GlEnum = 0x88E4;
pub(super) const GL_FLOAT: GlEnum = 0x1406;
pub(super) const GL_TRIANGLE_STRIP: GlEnum = 0x0005;
pub(super) const GL_PACK_ALIGNMENT: GlEnum = 0x0D05;
pub(super) const GL_UNPACK_ALIGNMENT: GlEnum = 0x0CF5;
pub(super) const GL_MAX_TEXTURE_SIZE: GlEnum = 0x0D33;
pub(super) const GL_RENDERER: GlEnum = 0x1F01;
pub(super) const GL_TEXTURE0: GlEnum = 0x84C0;
pub(super) const GL_BLEND: GlEnum = 0x0BE2;
pub(super) const GL_SCISSOR_TEST: GlEnum = 0x0C11;
pub(super) const GL_ONE: GlEnum = 1;
pub(super) const GL_SRC_ALPHA: GlEnum = 0x0302;
pub(super) const GL_ONE_MINUS_SRC_ALPHA: GlEnum = 0x0303;
pub(super) const GL_NO_ERROR: GlEnum = 0;

pub(super) const MAX_TAPS: usize = 33;
/// Hard-edged rounded-rect coverage, matching the CPU row insets: a fragment is inside when its
/// pixel centre lies within the signed distance of the rounded rectangle.
pub(super) const ROUNDED_SHADER: &str = "#ifdef GL_FRAGMENT_PRECISION_HIGH
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
pub(super) const MASK_SHADER: &str = "#ifdef GL_FRAGMENT_PRECISION_HIGH
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
pub(super) const IMAGE_SHADER: &str = "#ifdef GL_FRAGMENT_PRECISION_HIGH
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

/// Node backdrop composite: the blurred clip region (texel row 0 is the clip's first device row) is
/// written opaque inside the rounded node shape and discarded elsewhere. It mirrors
/// `pixels::inside_node_shape` at pixel centres, so the GPU and CPU keep the same pixels.
pub(super) const BACKDROP_SHADER: &str = "#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
uniform sampler2D u_texture;
uniform vec4 u_rect;
uniform vec4 u_node;
uniform float u_corner;
void main() {
    vec2 pixel = floor(gl_FragCoord.xy);
    float lx = pixel.x - u_node.x + 0.5;
    float ly = pixel.y - u_node.y + 0.5;
    float w = u_node.z;
    float h = u_node.w;
    if (lx < 0.0 || ly < 0.0 || lx > w || ly > h) {
        discard;
    }
    float r = min(u_corner, min(w / 2.0, h / 2.0));
    float cx = clamp(lx, r, w - r);
    float cy = clamp(ly, r, h - r);
    if ((lx - cx) * (lx - cx) + (ly - cy) * (ly - cy) > r * r) {
        discard;
    }
    vec2 uv = (pixel - u_rect.xy + 0.5) / u_rect.zw;
    gl_FragColor = vec4(texture2D(u_texture, uv).rgb, 1.0);
}
";

pub(super) const POSITION_ATTRIBUTE: GlUint = 0;

pub(super) const VERTEX_SHADER: &str = "attribute vec2 a_position;
varying vec2 v_uv;
void main() {
    v_uv = a_position * 0.5 + 0.5;
    gl_Position = vec4(a_position, 0.0, 1.0);
}
";

pub(super) const COPY_SHADER: &str = "#ifdef GL_FRAGMENT_PRECISION_HIGH
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

pub(super) const BLUR_SHADER: &str = "#ifdef GL_FRAGMENT_PRECISION_HIGH
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

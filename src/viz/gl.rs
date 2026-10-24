//! The OpenGL side: two textures that swap each frame (the previous picture
//! and the new one), a warp pass that bends the old picture into the new,
//! the waveform/shape pass on top, and a final pass to the window.

use std::sync::Arc;

use eframe::glow::{self, HasContext};

use super::draw::Vertex;
use super::preset::Look;

/// What one frame needs besides the geometry.
pub struct FrameParams {
    pub look: Look,
    pub time: f32,
    /// Window size in pixels, for the aspect ratio and the render size.
    pub aspect: f32,
    /// Largest render height in pixels (quality).
    pub max_height: u32,
    /// Show this frame as a hard cut: wipe the old picture first.
    pub clear: bool,
}

pub struct Gpu {
    gl: Arc<glow::Context>,
    warp: glow::Program,
    draw: glow::Program,
    comp: glow::Program,
    empty_vao: glow::VertexArray,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    targets: [Option<Target>; 2],
    size: (i32, i32),
    /// Which target holds the newest picture.
    current: usize,
    float: bool,
}

struct Target {
    texture: glow::Texture,
    framebuffer: glow::Framebuffer,
}

const HEADER_DESKTOP: &str = "#version 330 core\nprecision highp float;\n";
const HEADER_ES: &str = "#version 300 es\nprecision highp float;\n";

const FULLSCREEN_VS: &str = r#"
out vec2 uv;
void main() {
    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    uv = p;
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

const WARP_FS: &str = r#"
in vec2 uv;
out vec4 o;
uniform sampler2D prev;
uniform float aspect, time, zoom, rot, swirl, dx, dy, warp, wfreq, wspeed, decay, hue;

vec3 hue_rotate(vec3 c, float a) {
    const vec3 k = vec3(0.57735);
    float ca = cos(a), sa = sin(a);
    return c * ca + cross(k, c) * sa + k * dot(k, c) * (1.0 - ca);
}

void main() {
    vec2 p = (uv - 0.5) * vec2(aspect, 1.0);
    float r = length(p);
    float a = rot + swirl * r;
    float s = sin(a), c = cos(a);
    p = mat2(c, -s, s, c) * p;
    p /= zoom;
    p += warp * vec2(sin(p.y * wfreq + time * wspeed), cos(p.x * wfreq + time * wspeed * 1.17));
    p += vec2(dx, dy);
    vec2 src = p / vec2(aspect, 1.0) + 0.5;
    vec3 col = texture(prev, src).rgb * decay;
    col = hue_rotate(col, hue);
    // let near-black fade out for good instead of lingering
    col = max(col - 0.0015, 0.0);
    o = vec4(col, 1.0);
}
"#;

const DRAW_VS: &str = r#"
layout(location = 0) in vec2 pos;
layout(location = 1) in vec4 color;
out vec4 v;
void main() {
    v = color;
    gl_Position = vec4(pos, 0.0, 1.0);
}
"#;

const DRAW_FS: &str = r#"
in vec4 v;
out vec4 o;
void main() { o = v; }
"#;

const COMP_FS: &str = r#"
in vec2 uv;
out vec4 o;
uniform sampler2D tex;
uniform float aspect, mirror, kseg, bright, vignette, aberr, flash, invert;

void main() {
    vec2 u = uv;
    if (mirror > 0.5 && mirror < 1.5) { u.x = 0.5 - abs(u.x - 0.5); }
    else if (mirror > 1.5 && mirror < 2.5) { u.y = 0.5 - abs(u.y - 0.5); }
    else if (mirror > 2.5 && mirror < 3.5) { u = 0.5 - abs(u - 0.5); }
    else if (mirror > 3.5) {
        vec2 p = (u - 0.5) * vec2(aspect, 1.0);
        float r = length(p);
        float seg = 6.2831853 / kseg;
        float a = mod(atan(p.y, p.x), seg);
        a = abs(a - seg * 0.5);
        p = r * vec2(cos(a), sin(a));
        u = p / vec2(aspect, 1.0) + 0.5;
    }
    vec2 off = (u - 0.5) * aberr;
    vec3 col = vec3(texture(tex, u + off).r, texture(tex, u).g, texture(tex, u - off).b);
    col *= bright;
    float d = length((uv - 0.5) * vec2(aspect, 1.0)) * 1.15;
    col *= 1.0 - vignette * smoothstep(0.35, 1.0, d);
    col = mix(col, vec3(1.0) - col, invert);
    col += flash;
    o = vec4(clamp(col, 0.0, 1.0), 1.0);
}
"#;

impl Gpu {
    pub fn new(gl: Arc<glow::Context>) -> Result<Self, String> {
        let version = gl.version().clone();
        let header = if version.is_embedded {
            if version.major < 3 {
                return Err(format!("OpenGL ES {}.{} is too old (3.0 needed)", version.major, version.minor));
            }
            HEADER_ES
        } else {
            if (version.major, version.minor) < (3, 3) {
                return Err(format!("OpenGL {}.{} is too old (3.3 needed)", version.major, version.minor));
            }
            HEADER_DESKTOP
        };
        unsafe {
            let fullscreen = |fs: &str| compile(&gl, header, FULLSCREEN_VS, fs);
            let warp = fullscreen(WARP_FS)?;
            let comp = fullscreen(COMP_FS)?;
            let draw = compile(&gl, header, DRAW_VS, DRAW_FS)?;
            let empty_vao = gl.create_vertex_array()?;
            let vao = gl.create_vertex_array()?;
            let vbo = gl.create_buffer()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            let stride = std::mem::size_of::<Vertex>() as i32;
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 4, glow::FLOAT, false, stride, 8);
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            Ok(Self { gl, warp, draw, comp, empty_vao, vao, vbo, targets: [None, None], size: (0, 0), current: 0, float: true })
        }
    }

    /// Draw one frame into the window area `viewport` (x, y, width, height in
    /// pixels, as the egui callback gets it). `restore_fbo` is what to bind
    /// afterwards.
    pub fn render(
        &mut self,
        params: &FrameParams,
        vertices: &[Vertex],
        viewport: [i32; 4],
        restore_fbo: Option<glow::Framebuffer>,
    ) {
        let [vx, vy, vw, vh] = viewport;
        if vw < 2 || vh < 2 {
            return;
        }
        // render at the window's size, but never above the quality's height
        let scale = (params.max_height as f32 / vh as f32).min(1.0);
        let size = (((vw as f32 * scale) as i32).max(2) & !1, ((vh as f32 * scale) as i32).max(2) & !1);
        if size != self.size || self.targets[0].is_none() {
            self.resize(size);
        }
        let (src, dst) = (self.current, 1 - self.current);
        let (Some(a), Some(b)) = (&self.targets[src], &self.targets[dst]) else { return };
        let gl = &self.gl;
        let look = &params.look;
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.color_mask(true, true, true, true);
            gl.viewport(0, 0, size.0, size.1);

            // 1. the old picture, bent and faded, into the new target
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(b.framebuffer));
            gl.disable(glow::BLEND);
            if params.clear {
                gl.clear_color(0.0, 0.0, 0.0, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
            } else {
                gl.use_program(Some(self.warp));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(a.texture));
                let u = |name: &str, v: f32| {
                    if let Some(loc) = gl.get_uniform_location(self.warp, name) {
                        gl.uniform_1_f32(Some(&loc), v);
                    }
                };
                if let Some(loc) = gl.get_uniform_location(self.warp, "prev") {
                    gl.uniform_1_i32(Some(&loc), 0);
                }
                u("aspect", params.aspect);
                u("time", params.time);
                u("zoom", look.zoom);
                u("rot", look.rot);
                u("swirl", look.swirl);
                u("dx", look.dx);
                u("dy", look.dy);
                u("warp", look.warp);
                u("wfreq", look.warp_freq);
                u("wspeed", look.warp_speed);
                u("decay", look.decay);
                u("hue", look.hue_shift);
                gl.bind_vertex_array(Some(self.empty_vao));
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }

            // 2. the waveform and shapes on top
            if !vertices.is_empty() {
                gl.enable(glow::BLEND);
                gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
                gl.use_program(Some(self.draw));
                gl.bind_vertex_array(Some(self.vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
                let bytes = std::slice::from_raw_parts(vertices.as_ptr().cast::<u8>(), std::mem::size_of_val(vertices));
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STREAM_DRAW);
                gl.draw_arrays(glow::TRIANGLES, 0, vertices.len() as i32);
                gl.bind_buffer(glow::ARRAY_BUFFER, None);
                gl.disable(glow::BLEND);
            }

            // 3. to the window
            gl.bind_framebuffer(glow::FRAMEBUFFER, restore_fbo);
            gl.viewport(vx, vy, vw, vh);
            gl.use_program(Some(self.comp));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(b.texture));
            let u = |name: &str, v: f32| {
                if let Some(loc) = gl.get_uniform_location(self.comp, name) {
                    gl.uniform_1_f32(Some(&loc), v);
                }
            };
            if let Some(loc) = gl.get_uniform_location(self.comp, "tex") {
                gl.uniform_1_i32(Some(&loc), 0);
            }
            u("aspect", params.aspect);
            u("mirror", look.mirror);
            u("kseg", look.kaleido);
            u("bright", look.bright);
            u("vignette", look.vignette);
            u("aberr", look.aberration);
            u("flash", look.flash);
            u("invert", look.invert);
            gl.bind_vertex_array(Some(self.empty_vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);

            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
        }
        self.current = dst;
    }

    fn resize(&mut self, size: (i32, i32)) {
        unsafe {
            for slot in &mut self.targets {
                if let Some(t) = slot.take() {
                    t.delete(&self.gl);
                }
            }
            let make = |float| (Target::new(&self.gl, size, float), Target::new(&self.gl, size, float));
            let (mut a, mut b) = make(self.float);
            if (a.is_none() || b.is_none()) && self.float {
                // no float render targets (some GLES drivers): 8 bits will do
                for t in [a.take(), b.take()].into_iter().flatten() {
                    t.delete(&self.gl);
                }
                self.float = false;
                (a, b) = make(false);
            }
            self.targets = [a, b];
            // start from black
            for t in self.targets.iter().flatten() {
                self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(t.framebuffer));
                self.gl.clear_color(0.0, 0.0, 0.0, 1.0);
                self.gl.clear(glow::COLOR_BUFFER_BIT);
            }
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        self.size = size;
        self.current = 0;
    }

    /// Give everything back; the context must be current.
    pub fn destroy(self) {
        unsafe {
            for t in self.targets.into_iter().flatten() {
                t.delete(&self.gl);
            }
            self.gl.delete_program(self.warp);
            self.gl.delete_program(self.draw);
            self.gl.delete_program(self.comp);
            self.gl.delete_vertex_array(self.empty_vao);
            self.gl.delete_vertex_array(self.vao);
            self.gl.delete_buffer(self.vbo);
        }
    }
}

impl Target {
    unsafe fn new(gl: &glow::Context, size: (i32, i32), float: bool) -> Option<Self> {
        unsafe {
            let texture = gl.create_texture().ok()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            let (internal, kind) = if float { (glow::RGBA16F as i32, glow::HALF_FLOAT) } else { (glow::RGBA8 as i32, glow::UNSIGNED_BYTE) };
            gl.tex_image_2d(glow::TEXTURE_2D, 0, internal, size.0, size.1, 0, glow::RGBA, kind, glow::PixelUnpackData::Slice(None));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::MIRRORED_REPEAT as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::MIRRORED_REPEAT as i32);
            let framebuffer = gl.create_framebuffer().ok()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
            let ok = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            if ok {
                Some(Self { texture, framebuffer })
            } else {
                gl.delete_framebuffer(framebuffer);
                gl.delete_texture(texture);
                None
            }
        }
    }

    unsafe fn delete(self, gl: &glow::Context) {
        unsafe {
            gl.delete_framebuffer(self.framebuffer);
            gl.delete_texture(self.texture);
        }
    }
}

unsafe fn compile(gl: &glow::Context, header: &str, vertex: &str, fragment: &str) -> Result<glow::Program, String> {
    unsafe {
        let program = gl.create_program()?;
        let mut shaders = Vec::new();
        for (kind, source) in [(glow::VERTEX_SHADER, vertex), (glow::FRAGMENT_SHADER, fragment)] {
            let shader = gl.create_shader(kind)?;
            gl.shader_source(shader, &format!("{header}{source}"));
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                let log = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                gl.delete_program(program);
                return Err(format!("shader error: {log}"));
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }
        gl.link_program(program);
        for shader in shaders {
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }
        if !gl.get_program_link_status(program) {
            let log = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(format!("link error: {log}"));
        }
        Ok(program)
    }
}

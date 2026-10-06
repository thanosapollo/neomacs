// Native model view: depth-tested mesh pass (design §4.5).
//
// Vertex pulling from storage buffers: the face id is vertex_index / 3, so no
// PRIMITIVE_INDEX feature is needed. Camera and lighting follow canvas-3d,
// which stored Gnosis views were authored against.

struct Camera {
    // Rows of R = Ry(yaw) * Rx(pitch); w unused.
    r0: vec4<f32>,
    r1: vec4<f32>,
    r2: vec4<f32>,
    // xy: zoom * 0.8 * fit; z: selected mesh + 1 (0 = none); w unused.
    scale: vec4<f32>,
    base: vec4<f32>,
    marked: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<storage, read> positions: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> indices: array<u32>;
// xyz: unit face normal; w: mesh index.
@group(0) @binding(3) var<storage, read> faces: array<vec4<f32>>;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) @interpolate(flat) mesh: f32,
};

fn rotate(v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(camera.r0.xyz, v), dot(camera.r1.xyz, v), dot(camera.r2.xyz, v));
}

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> VertexOut {
    let face = faces[vertex / 3u];
    let q = rotate(positions[indices[vertex]].xyz);
    var out: VertexOut;
    out.clip = vec4<f32>(q.xy * camera.scale.xy, (1.0 - q.z * 0.25) * 0.5, 1.0);
    out.normal = rotate(face.xyz);
    out.mesh = face.w;
    return out;
}

// canvas-3d wrote these values straight into an RGBA8 texture; the surface
// texture is sRGB, so decode first and the stored bytes match canvas-3d.
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let light = 0.28 + 0.72 * abs(dot(normalize(in.normal), normalize(vec3<f32>(-0.4, 0.6, 1.0))));
    let selected = camera.scale.z > 0.5 && abs(in.mesh + 1.0 - camera.scale.z) < 0.5;
    let colour = select(camera.base.rgb, camera.marked.rgb, selected);
    return vec4<f32>(srgb_to_linear(colour * light), 1.0);
}

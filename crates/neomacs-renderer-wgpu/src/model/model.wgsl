// Native model view: depth-tested mesh pass and 1x1 pick pass (design §4.5, §5.4).
//
// Vertex pulling from storage buffers: the face id is vertex_index / 3, so no
// PRIMITIVE_INDEX feature is needed. Camera, lighting and highlight rules
// follow canvas-3d (render.py), which stored Gnosis views were authored
// against.

struct Camera {
    // Rows of R = Ry(yaw) * Rx(pitch); w unused.
    r0: vec4<f32>,
    r1: vec4<f32>,
    r2: vec4<f32>,
    // xy: zoom * 0.8 * fit; zw unused.
    scale: vec4<f32>,
    // Sub-texel pick projection: clip.xy = (q.xy * scale - pick.xy) * pick.zw.
    // xy: NDC centre of the picked texel; zw: texture size in texels.
    // The colour pass uses (0, 0, 1, 1).
    pick: vec4<f32>,
    base: vec4<f32>,
    marked: vec4<f32>,
    // x: painted whole mesh + 1 (0 none: a target object, or the selection
    // when there is no target); y: point-target mesh + 1 (0 none); z: 1 when
    // the region marks apply.
    paint: vec4<f32>,
    // Point target: xyz centre and w radius, in the normalised frame.
    point: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<storage, read> positions: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> indices: array<u32>;
// xyz: unit face normal; w: mesh index.
@group(0) @binding(3) var<storage, read> faces: array<vec4<f32>>;
// One bit per global face: region target membership.
@group(0) @binding(4) var<storage, read> marks: array<u32>;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) @interpolate(flat) mesh: f32,
    // Unrotated normalised position (canvas-3d `normalized_point`).
    @location(2) pos: vec3<f32>,
    @location(3) @interpolate(flat) face: u32,
    @location(4) bary: vec3<f32>,
};

fn rotate(v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(camera.r0.xyz, v), dot(camera.r1.xyz, v), dot(camera.r2.xyz, v));
}

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> VertexOut {
    let face_id = vertex / 3u;
    let face = faces[face_id];
    let p = positions[indices[vertex]].xyz;
    let q = rotate(p);
    var out: VertexOut;
    let xy = (q.xy * camera.scale.xy - camera.pick.xy) * camera.pick.zw;
    out.clip = vec4<f32>(xy, (1.0 - q.z * 0.25) * 0.5, 1.0);
    out.normal = rotate(face.xyz);
    out.mesh = face.w;
    out.pos = p;
    out.face = face_id;
    let corner = vertex % 3u;
    out.bary = vec3<f32>(f32(corner == 0u), f32(corner == 1u), f32(corner == 2u));
    return out;
}

// canvas-3d wrote these values straight into an RGBA8 texture; the surface
// texture is sRGB, so decode first and the stored bytes match canvas-3d.
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, c <= vec3<f32>(0.04045));
}

fn is_mesh(mesh: f32, plus_one: f32) -> bool {
    return plus_one > 0.5 && abs(mesh + 1.0 - plus_one) < 0.5;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let light = 0.28 + 0.72 * abs(dot(normalize(in.normal), normalize(vec3<f32>(-0.4, 0.6, 1.0))));
    var marked = is_mesh(in.mesh, camera.paint.x);
    if (is_mesh(in.mesh, camera.paint.y)) {
        marked = marked || distance(in.pos, camera.point.xyz) <= camera.point.w;
    }
    if (camera.paint.z > 0.5) {
        marked = marked || ((marks[in.face / 32u] >> (in.face % 32u)) & 1u) == 1u;
    }
    let colour = select(camera.base.rgb, camera.marked.rgb, marked);
    return vec4<f32>(srgb_to_linear(colour * light), 1.0);
}

struct PickOut {
    // (mesh + 1, global face + 1); 0 is background.
    @location(0) ids: vec2<u32>,
    // Barycentric weights of corners 1 and 2.
    @location(1) bary: vec2<f32>,
};

@fragment
fn fs_pick(in: VertexOut) -> PickOut {
    var out: PickOut;
    out.ids = vec2<u32>(u32(in.mesh + 0.5) + 1u, in.face + 1u);
    out.bary = in.bary.yz;
    return out;
}

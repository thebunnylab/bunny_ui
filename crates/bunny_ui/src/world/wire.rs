//! The bytes a GPU tier reads for a world frame, in one layout for every
//! tier.
//!
//! Every block is built of 16-byte rows — four `f32`, or four `u32` —
//! and holds no scalar arrays. Under that rule the std140 and std430
//! layouts of GLSL, the constant buffers and structured buffers of HLSL,
//! the `constant` and `device` structs of MSL and this module agree byte
//! for byte. The writers below are the layout; the tests pin each
//! offset, and a tier that reads one field from the wrong place fails
//! its parity test on the first frame.
//!
//! All values are little-endian. Positions and directions are in the
//! EYE's space: the camera stands at the origin and looks down −Z, so the
//! GPU never sees a large number.

/// Rows of four.
pub type Row = [f32; 4];

/// The per-frame block: lens, viewport, lights, tone. 704 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct FrameUniforms {
    /// The projection, column-major: clip = this × eye-space position.
    pub clip_from_eye: [f32; 16],
    /// `(width px, height px, 1/width, 1/height)`.
    pub viewport: Row,
    /// `(device pixels per point, engine time, MSAA samples, lens)`;
    /// lens 0 = perspective, 1 = parallel.
    pub params: Row,
    /// The section plane in the eye's space, `(n, d)`; what lies at
    /// `n·p + d < 0` is cut away. All zero = no section.
    pub section: Row,
    /// The unit direction TOWARD the sun, in the eye's space; `w` = 1
    /// when the world has a sun.
    pub sun_direction: Row,
    /// The sun's light, linear, intensity folded in.
    pub sun_color: Row,
    /// The sky's light from above, linear.
    pub sky_color: Row,
    /// The light bounced from the ground, linear.
    pub ground_color: Row,
    /// The world's up in the eye's space; `w` = the eye's height above
    /// the ground plane.
    pub sky_up: Row,
    /// `(exposure, knee, dither, grade)`. The dither is 0 for none, else
    /// one plus the phase of its pattern, `x + 4·y` (each in `0..4`), added
    /// to the pixel's place; the grade is where its points start in the
    /// part table plus one, 0 for none.
    pub tone: Row,
    /// The sun's shadow map from the eye's space, column-major:
    /// `(u, v, depth, 1)`, with `u` and `v` across the map (`v = 0` at
    /// its first row) and the depth from the sun in `[0, 1]`.
    pub shadow_from_eye: [f32; 16],
    /// `(1/size, normal offset in meters, depth bias, taps)`; taps 0 =
    /// no shadow, else 1, 4 or 9 lookups.
    pub shadow: Row,
    /// `(point lights, 1 when a floor map is laid, 0, 0)`.
    pub counts: Row,
    /// Eye-space position; `w` = range in meters.
    pub point_position: [Row; MAX_POINTS],
    /// Linear light, intensity folded in; `w` = source radius.
    pub point_color: [Row; MAX_POINTS],
    /// The light groups each point light reaches, one `u32` each.
    pub point_groups: [[u32; 4]; MAX_POINTS / 4],
    /// The floor map's `u` and `v` of an eye-space point:
    /// `u = floor_u · (p, 1)`.
    pub floor_u: Row,
    pub floor_v: Row,
    /// The second shadow map, of what moves (a figure that walks), from
    /// the eye's space: as [`FrameUniforms::shadow_from_eye`].
    pub moving_from_eye: [f32; 16],
    /// As [`FrameUniforms::shadow`], for the second map; taps 0 = none.
    pub moving: Row,
}

/// Point lights a frame carries.
pub const MAX_POINTS: usize = 8;

/// Byte offsets of [`FrameUniforms`].
pub mod frame_offset {
    pub const CLIP_FROM_EYE: usize = 0;
    pub const VIEWPORT: usize = 64;
    pub const PARAMS: usize = 80;
    pub const SECTION: usize = 96;
    pub const SUN_DIRECTION: usize = 112;
    pub const SUN_COLOR: usize = 128;
    pub const SKY_COLOR: usize = 144;
    pub const GROUND_COLOR: usize = 160;
    pub const SKY_UP: usize = 176;
    pub const TONE: usize = 192;
    pub const SHADOW_FROM_EYE: usize = 208;
    pub const SHADOW: usize = 272;
    pub const COUNTS: usize = 288;
    pub const POINT_POSITION: usize = 304;
    pub const POINT_COLOR: usize = 432;
    pub const POINT_GROUPS: usize = 560;
    pub const FLOOR_U: usize = 592;
    pub const FLOOR_V: usize = 608;
    pub const MOVING_FROM_EYE: usize = 624;
    pub const MOVING: usize = 688;
    pub const SIZE: usize = 704;
}

/// One instance of a mesh or a set of lines. 128 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct InstanceData {
    /// The rows of the eye-from-model matrix: `x' = rows[0] · (x, y, z, 1)`.
    pub eye_from_model: [Row; 3],
    /// The rows of the normal matrix, eye space (`w` unused).
    pub normal: [Row; 3],
    /// Multiplies every material colour of the instance (sRGB).
    pub tint: [u8; 4],
    /// Where the instance's materials start in the frame's material table.
    pub material_base: u32,
    /// The entity's slot, for an id pass.
    pub entity: u32,
    /// [`instance_flag`] bits.
    pub flags: u32,
    /// Multiplies the light the instance takes from the sky and the
    /// ground (sRGB): the ambient of the room it stands in.
    pub ambient: [u8; 4],
    /// Where the states of the instance's parts start in the frame's part
    /// table, one a part its mesh names; [`NO_PARTS`] for none.
    pub part_base: u32,
    /// Where the instance's palette starts in the frame's joint rows,
    /// three rows a joint; [`NO_JOINTS`] for a mesh at rest.
    pub joint_base: u32,
    /// The weights of its mesh's morph targets, two 16-bit shares
    /// ([`super::mesh::pack_morph`]).
    pub morph: u32,
}

/// The `part_base` of an instance whose parts all keep their own look.
pub const NO_PARTS: u32 = u32::MAX;

/// The `joint_base` of an instance that hangs from no joints.
pub const NO_JOINTS: u32 = u32::MAX;

/// One part's state in the frame's part table: its tint (sRGB) and, in
/// the fourth byte, 255 when it shows and 0 when it hides.
pub type PartRow = [u8; 4];

/// Byte offsets of [`InstanceData`].
pub mod instance_offset {
    pub const EYE_FROM_MODEL: usize = 0;
    pub const NORMAL: usize = 48;
    pub const TINT: usize = 96;
    pub const MATERIAL_BASE: usize = 100;
    pub const ENTITY: usize = 104;
    pub const FLAGS: usize = 108;
    pub const AMBIENT: usize = 112;
    pub const PART_BASE: usize = 116;
    pub const JOINT_BASE: usize = 120;
    pub const MORPH: usize = 124;
    pub const SIZE: usize = 128;
}

/// Bits of [`InstanceData::flags`].
pub mod instance_flag {
    pub const CASTS_SHADOW: u32 = 1;
    /// The instance's surfaces read the floor map.
    pub const FLOOR_MAP: u32 = 2;
    /// Where the entity's light group (0 to 31) lies: its surfaces answer
    /// to the point lights whose mask holds bit `group`.
    pub const GROUP_SHIFT: u32 = 8;
    pub const GROUP_MASK: u32 = 31;
}

/// One material of the frame's table. 64 bytes; a tier stores the table
/// in a data texture of four RGBA32F texels per material
/// ([`MATERIALS_PER_ROW`] materials a row).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct MaterialData {
    /// Linear colour; `w` = alpha.
    pub base: Row,
    /// The back's linear colour; `w` = 1 when the back is drawn.
    pub back: Row,
    /// Linear light given off, strength folded in.
    pub emissive: Row,
    /// `(roughness, metallic, mask cutoff, flags)`; flags as
    /// [`material_flag`] bits, stored as an `f32` integer.
    pub params: Row,
}

/// Materials in one row of the material texture.
pub const MATERIALS_PER_ROW: usize = 1024;

/// Bits of [`MaterialData::params`]`[3]`.
pub mod material_flag {
    pub const UNLIT: u32 = 1;
    pub const TEXTURED: u32 = 2;
    pub const MASK: u32 = 4;
}

/// One caster of the sun's shadow map: the three rows of its
/// map-from-model matrix, `x' = rows[0] · (x, y, z, 1)` — `x` and `y`
/// across the map in `[-1, 1]` (`y` up), `z` the depth from the sun in
/// `[0, 1]` — and where the states of its parts start in the frame's part
/// table (a part that hides casts nothing), where its palette starts for
/// a skinned caster, and its morph weights. 64 bytes: the rows, the part
/// base, the joint base, the morph weights, a zero word.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowCaster {
    pub rows: [Row; 3],
    pub part_base: u32,
    pub joint_base: u32,
    pub morph: u32,
}

/// Bytes of one [`ShadowCaster`].
pub const SHADOW_INSTANCE_STRIDE: usize = 64;

/// How a set of lines is drawn. 32 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct LineStyleData {
    /// Linear colour, straight alpha.
    pub color: Row,
    /// `(width px, dash px, gap px, depth mode)`; dash 0 = solid; depth
    /// mode 0 = tested, 1 = hidden only, 2 = always.
    pub shape: Row,
}

/// Bytes of one segment in a segment buffer: two `f32 × 3` ends in the
/// model's space.
pub const SEGMENT_STRIDE: usize = 24;

fn put_row(out: &mut Vec<u8>, row: &Row) {
    for value in row {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

fn put_matrix(out: &mut Vec<u8>, matrix: &[f32; 16]) {
    for value in matrix {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

impl FrameUniforms {
    pub fn write(&self, out: &mut Vec<u8>) {
        put_matrix(out, &self.clip_from_eye);
        for row in [&self.viewport, &self.params, &self.section, &self.sun_direction, &self.sun_color, &self.sky_color, &self.ground_color, &self.sky_up, &self.tone] {
            put_row(out, row);
        }
        put_matrix(out, &self.shadow_from_eye);
        put_row(out, &self.shadow);
        put_row(out, &self.counts);
        for row in self.point_position.iter().chain(self.point_color.iter()) {
            put_row(out, row);
        }
        for group in &self.point_groups {
            for value in group {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
        put_row(out, &self.floor_u);
        put_row(out, &self.floor_v);
        put_matrix(out, &self.moving_from_eye);
        put_row(out, &self.moving);
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(frame_offset::SIZE);
        self.write(&mut out);
        out
    }
}

impl InstanceData {
    pub fn write(&self, out: &mut Vec<u8>) {
        for row in self.eye_from_model.iter().chain(self.normal.iter()) {
            put_row(out, row);
        }
        out.extend_from_slice(&self.tint);
        out.extend_from_slice(&self.material_base.to_le_bytes());
        out.extend_from_slice(&self.entity.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        out.extend_from_slice(&self.ambient);
        out.extend_from_slice(&self.part_base.to_le_bytes());
        out.extend_from_slice(&self.joint_base.to_le_bytes());
        out.extend_from_slice(&self.morph.to_le_bytes());
    }
}

impl MaterialData {
    pub fn write(&self, out: &mut Vec<u8>) {
        for row in [&self.base, &self.back, &self.emissive, &self.params] {
            put_row(out, row);
        }
    }
}

impl LineStyleData {
    pub fn write(&self, out: &mut Vec<u8>) {
        put_row(out, &self.color);
        put_row(out, &self.shape);
    }
}

/// Every instance, back to back.
pub fn instance_bytes(instances: &[InstanceData]) -> Vec<u8> {
    let mut out = Vec::with_capacity(instances.len() * instance_offset::SIZE);
    for instance in instances {
        instance.write(&mut out);
    }
    out
}

/// Every caster of the shadow map, back to back.
pub fn shadow_instance_bytes(instances: &[ShadowCaster]) -> Vec<u8> {
    let mut out = Vec::with_capacity(instances.len() * SHADOW_INSTANCE_STRIDE);
    for caster in instances {
        for row in &caster.rows {
            put_row(&mut out, row);
        }
        for word in [caster.part_base, caster.joint_base, caster.morph, 0] {
            out.extend_from_slice(&word.to_le_bytes());
        }
    }
    out
}

/// The part table, back to back: four bytes a part.
pub fn part_bytes(rows: &[PartRow]) -> Vec<u8> {
    rows.iter().flatten().copied().collect()
}

/// Every material, back to back: four texels of a data texture each.
pub fn material_bytes(materials: &[MaterialData]) -> Vec<u8> {
    let mut out = Vec::with_capacity(materials.len() * 64);
    for material in materials {
        material.write(&mut out);
    }
    out
}

/// Every line style, back to back.
pub fn line_style_bytes(styles: &[LineStyleData]) -> Vec<u8> {
    let mut out = Vec::with_capacity(styles.len() * 32);
    for style in styles {
        style.write(&mut out);
    }
    out
}

/// The segments of a set of lines, [`SEGMENT_STRIDE`] bytes each.
pub fn segment_bytes(segments: &[[[f32; 3]; 2]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(segments.len() * SEGMENT_STRIDE);
    for segment in segments {
        for value in segment.iter().flatten() {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f32_at(bytes: &[u8], at: usize) -> f32 {
        f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn the_frame_block_keeps_its_offsets() {
        let mut uniforms = FrameUniforms { clip_from_eye: [0.0; 16], ..FrameUniforms::default() };
        uniforms.clip_from_eye[15] = 1.5;
        uniforms.viewport = [1.0, 2.0, 3.0, 4.0];
        uniforms.params[3] = 5.0;
        uniforms.section[0] = 6.0;
        uniforms.sun_direction[1] = 7.0;
        uniforms.sun_color[2] = 8.0;
        uniforms.sky_color[3] = 9.0;
        uniforms.ground_color[0] = 10.0;
        uniforms.sky_up[3] = 11.0;
        uniforms.tone[0] = 12.0;
        uniforms.shadow_from_eye[0] = 13.0;
        uniforms.shadow[3] = 14.0;
        uniforms.counts[0] = 15.0;
        uniforms.point_position[7][3] = 16.0;
        uniforms.point_color[0][0] = 17.0;
        uniforms.point_groups[1][3] = 0xDEAD_BEEF;
        uniforms.floor_u[3] = 18.0;
        uniforms.floor_v[0] = 19.0;
        uniforms.moving_from_eye[15] = 20.0;
        uniforms.moving[3] = 21.0;
        let bytes = uniforms.to_bytes();
        assert_eq!(bytes.len(), frame_offset::SIZE);
        use frame_offset as at;
        assert_eq!(f32_at(&bytes, at::CLIP_FROM_EYE + 60), 1.5);
        assert_eq!(f32_at(&bytes, at::VIEWPORT + 4), 2.0);
        assert_eq!(f32_at(&bytes, at::PARAMS + 12), 5.0);
        assert_eq!(f32_at(&bytes, at::SECTION), 6.0);
        assert_eq!(f32_at(&bytes, at::SUN_DIRECTION + 4), 7.0);
        assert_eq!(f32_at(&bytes, at::SUN_COLOR + 8), 8.0);
        assert_eq!(f32_at(&bytes, at::SKY_COLOR + 12), 9.0);
        assert_eq!(f32_at(&bytes, at::GROUND_COLOR), 10.0);
        assert_eq!(f32_at(&bytes, at::SKY_UP + 12), 11.0);
        assert_eq!(f32_at(&bytes, at::TONE), 12.0);
        assert_eq!(f32_at(&bytes, at::SHADOW_FROM_EYE), 13.0);
        assert_eq!(f32_at(&bytes, at::SHADOW + 12), 14.0);
        assert_eq!(f32_at(&bytes, at::COUNTS), 15.0);
        assert_eq!(f32_at(&bytes, at::POINT_POSITION + 7 * 16 + 12), 16.0);
        assert_eq!(f32_at(&bytes, at::POINT_COLOR), 17.0);
        assert_eq!(u32_at(&bytes, at::POINT_GROUPS + 16 + 12), 0xDEAD_BEEF);
        assert_eq!(f32_at(&bytes, at::FLOOR_U + 12), 18.0);
        assert_eq!(f32_at(&bytes, at::FLOOR_V), 19.0);
        assert_eq!(f32_at(&bytes, at::MOVING_FROM_EYE + 60), 20.0);
        assert_eq!(f32_at(&bytes, at::MOVING + 12), 21.0);
    }

    #[test]
    fn an_instance_keeps_its_offsets() {
        let instance = InstanceData {
            eye_from_model: [[1.0, 0.0, 0.0, 2.0], [0.0, 1.0, 0.0, 3.0], [0.0, 0.0, 1.0, 4.0]],
            normal: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 9.0, 0.0]],
            tint: [10, 20, 30, 40],
            material_base: 77,
            entity: 88,
            flags: instance_flag::CASTS_SHADOW,
            ambient: [50, 60, 70, 255],
            part_base: 99,
            joint_base: NO_JOINTS,
            morph: 0,
        };
        let bytes = instance_bytes(&[instance, instance]);
        assert_eq!(bytes.len(), 2 * instance_offset::SIZE);
        use instance_offset as at;
        assert_eq!(f32_at(&bytes, at::EYE_FROM_MODEL + 12), 2.0);
        assert_eq!(f32_at(&bytes, at::EYE_FROM_MODEL + 44), 4.0);
        assert_eq!(f32_at(&bytes, at::NORMAL + 40), 9.0);
        assert_eq!(&bytes[at::TINT..at::TINT + 4], &[10, 20, 30, 40]);
        assert_eq!(u32_at(&bytes, at::MATERIAL_BASE), 77);
        assert_eq!(u32_at(&bytes, at::ENTITY), 88);
        assert_eq!(u32_at(&bytes, at::FLAGS), 1);
        assert_eq!(&bytes[at::AMBIENT..at::AMBIENT + 4], &[50, 60, 70, 255]);
        assert_eq!(u32_at(&bytes, at::PART_BASE), 99);
        assert_eq!(u32_at(&bytes, at::JOINT_BASE), NO_JOINTS);
        assert_eq!(u32_at(&bytes, at::MORPH), 0);
        assert_eq!(u32_at(&bytes, at::SIZE + at::MATERIAL_BASE), 77, "the second instance starts at 128");
    }

    #[test]
    fn materials_styles_and_segments_keep_their_strides() {
        let material = MaterialData { params: [0.5, 0.25, 0.0, 3.0], ..MaterialData::default() };
        let bytes = material_bytes(&[material, material]);
        assert_eq!(bytes.len(), 128);
        assert_eq!(f32_at(&bytes, 64 + 48 + 4), 0.25);
        let style = LineStyleData { color: [1.0, 0.0, 0.0, 1.0], shape: [2.0, 6.0, 3.0, 1.0] };
        let bytes = line_style_bytes(&[style]);
        assert_eq!(bytes.len(), 32);
        assert_eq!(f32_at(&bytes, 16 + 12), 1.0);
        let bytes = segment_bytes(&[[[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]]);
        assert_eq!(bytes.len(), SEGMENT_STRIDE);
        assert_eq!(f32_at(&bytes, 20), 6.0);
        let caster = ShadowCaster { rows: [[1.0, 0.0, 0.0, 0.5], [0.0, 1.0, 0.0, 0.25], [0.0, 0.0, 1.0, 0.125]], part_base: 7, joint_base: 9, morph: 11 };
        let bytes = shadow_instance_bytes(&[caster, caster]);
        assert_eq!(bytes.len(), 2 * SHADOW_INSTANCE_STRIDE);
        assert_eq!(f32_at(&bytes, SHADOW_INSTANCE_STRIDE + 44), 0.125);
        assert_eq!(u32_at(&bytes, SHADOW_INSTANCE_STRIDE + 48), 7);
        assert_eq!(u32_at(&bytes, SHADOW_INSTANCE_STRIDE + 52), 9, "the joint base beside the part base");
        assert_eq!(u32_at(&bytes, SHADOW_INSTANCE_STRIDE + 56), 11, "and the morph weights");
    }
}

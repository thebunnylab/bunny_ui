//! Skins and clips: a figure whose vertices hang from joints, and the
//! motion of those joints over time.
//!
//! A [`Skeleton`] is a tree of nodes in their rest pose — the nodes of an
//! asset, or a tree an app builds. A [`Skin`] names the nodes a mesh's
//! vertices hang from (its joints) and, for each, the matrix that takes a
//! vertex from the mesh's space into the joint's (its inverse bind). A
//! [`Clip`] moves the nodes: keys of translation, rotation and scale,
//! held or blended. An entity plays a clip
//! ([`super::store::Entity::play`]), and every skinned entity under it
//! moves with it: a figure and the clothes it wears read one pose.
//!
//! The pose is worked out on the CPU once a frame for each entity that
//! plays, in `f64` with the four operations and square roots alone, so a
//! pose is the same bytes on every machine. Between two keys a rotation
//! turns along the straight blend of the two, made unit again: for keys
//! a thirtieth of a second apart, the arc's own path. Each skinned
//! instance takes its palette — three rows a joint, the joint's matrix
//! times its inverse bind, in the mesh's space — and the GPU and the
//! CPU's rasterizer read the same rows.

use std::rc::Rc;

use super::math::{Mat4, Quat, Transform, Vec3, vec3};
use super::next_id;
use super::wire::Row;

/// The most joints a skin may name: a vertex names its joints in a byte.
pub const MAX_JOINTS: usize = 256;

/// One node of a skeleton: its parent, and where it stands at rest,
/// relative to the parent.
#[derive(Clone, Debug, PartialEq)]
pub struct SkeletonNode {
    pub name: Option<Rc<str>>,
    /// Into the skeleton's nodes; `None` for a node at the top.
    pub parent: Option<u32>,
    pub rest: Transform,
}

/// A tree of nodes in their rest pose. Cheap to clone and to share
/// between skins.
#[derive(Clone, Debug)]
pub struct Skeleton(Rc<SkeletonInner>);

#[derive(Debug)]
struct SkeletonInner {
    id: u64,
    nodes: Vec<SkeletonNode>,
    /// Every node after its parent: the order a pose is worked out in.
    order: Vec<u32>,
}

impl PartialEq for Skeleton {
    fn eq(&self, other: &Skeleton) -> bool {
        self.0.id == other.0.id
    }
}

impl Skeleton {
    /// A skeleton of `nodes`. A parent that names no node, or a loop of
    /// parents, leaves the node at the top.
    pub fn new(mut nodes: Vec<SkeletonNode>) -> Skeleton {
        let count = nodes.len();
        for node in &mut nodes {
            if node.parent.is_some_and(|parent| parent as usize >= count) {
                node.parent = None;
            }
        }
        // each node after its parent; a node in a loop goes to the top
        let mut order = Vec::with_capacity(count);
        let mut placed = vec![false; count];
        let mut visiting = vec![false; count];
        for start in 0..count {
            let mut chain = Vec::new();
            let mut at = Some(start as u32);
            while let Some(node) = at {
                if placed[node as usize] {
                    break;
                }
                if visiting[node as usize] {
                    nodes[node as usize].parent = None;
                    break;
                }
                visiting[node as usize] = true;
                chain.push(node);
                at = nodes[node as usize].parent;
            }
            for &node in chain.iter().rev() {
                placed[node as usize] = true;
                order.push(node);
            }
        }
        Skeleton(Rc::new(SkeletonInner { id: next_id(), nodes, order }))
    }

    pub fn id(&self) -> u64 {
        self.0.id
    }

    pub fn nodes(&self) -> &[SkeletonNode] {
        &self.0.nodes
    }

    /// The node named `name`.
    pub fn node(&self, name: &str) -> Option<u32> {
        self.0.nodes.iter().position(|node| node.name.as_deref() == Some(name)).map(|at| at as u32)
    }

    /// Where every node stands at `time`, in the skeleton's space: the
    /// rest pose, moved by `play`'s clip where it has keys.
    pub fn pose(&self, play: Option<&Play>, time: f64) -> Vec<Mat4> {
        let nodes = &self.0.nodes;
        let mut local: Vec<Transform> = nodes.iter().map(|node| node.rest).collect();
        if let Some(play) = play {
            let at = play.clip_time(time);
            for channel in &play.clip.0.channels {
                let Some(node) = local.get_mut(channel.node as usize) else { continue };
                match channel.path {
                    Path::Translation => node.translation = channel.vector(at),
                    Path::Scale => node.scale = channel.vector(at),
                    Path::Rotation => node.rotation = channel.rotation(at),
                }
            }
        }
        let mut global = vec![Mat4::IDENTITY; nodes.len()];
        for &node in &self.0.order {
            let matrix = local[node as usize].matrix();
            global[node as usize] = match nodes[node as usize].parent {
                Some(parent) => global[parent as usize] * matrix,
                None => matrix,
            };
        }
        global
    }
}

/// How a mesh hangs from a skeleton: the nodes its vertices name, by the
/// joint number a vertex carries, each with its inverse bind; and the
/// mesh's own place in the skeleton's space.
#[derive(Clone, Debug)]
pub struct Skin(Rc<SkinInner>);

#[derive(Debug)]
struct SkinInner {
    id: u64,
    skeleton: Skeleton,
    joints: Vec<u32>,
    inverse_bind: Vec<Mat4>,
    /// From the skeleton's space into the mesh's.
    mesh_from_skeleton: Mat4,
}

impl PartialEq for Skin {
    fn eq(&self, other: &Skin) -> bool {
        self.0.id == other.0.id
    }
}

impl Skin {
    /// A skin of `joints` (nodes of `skeleton`) with their inverse binds;
    /// a joint without one takes the identity. At most [`MAX_JOINTS`]
    /// joints: past that, `None`.
    pub fn new(skeleton: Skeleton, joints: Vec<u32>, inverse_bind: Vec<Mat4>) -> Option<Skin> {
        if joints.len() > MAX_JOINTS {
            return None;
        }
        let mut inverse_bind = inverse_bind;
        inverse_bind.resize(joints.len(), Mat4::IDENTITY);
        Some(Skin(Rc::new(SkinInner { id: next_id(), skeleton, joints, inverse_bind, mesh_from_skeleton: Mat4::IDENTITY })))
    }

    /// The same skin, for a mesh that stands at `skeleton_from_mesh` in
    /// the skeleton's space (a glTF node that holds a skinned mesh).
    pub fn placed(&self, skeleton_from_mesh: &Mat4) -> Skin {
        let inner = &self.0;
        Skin(Rc::new(SkinInner {
            id: next_id(),
            skeleton: inner.skeleton.clone(),
            joints: inner.joints.clone(),
            inverse_bind: inner.inverse_bind.clone(),
            mesh_from_skeleton: skeleton_from_mesh.inverse().unwrap_or(Mat4::IDENTITY),
        }))
    }

    pub fn id(&self) -> u64 {
        self.0.id
    }

    pub fn skeleton(&self) -> &Skeleton {
        &self.0.skeleton
    }

    pub fn joints(&self) -> &[u32] {
        &self.0.joints
    }

    /// The palette of `pose` (a pose of this skin's skeleton): three rows
    /// a joint, the matrix that takes a vertex of the mesh at rest to the
    /// mesh's space at the pose, `x' = rows[3j] · (x, y, z, 1)`.
    pub fn palette(&self, pose: &[Mat4]) -> Vec<Row> {
        let inner = &self.0;
        let mut rows = Vec::with_capacity(inner.joints.len() * 3);
        for (joint, inverse_bind) in inner.joints.iter().zip(&inner.inverse_bind) {
            let node = pose.get(*joint as usize).copied().unwrap_or(Mat4::IDENTITY);
            let matrix = inner.mesh_from_skeleton * node * *inverse_bind;
            for row in 0..3 {
                rows.push([0, 1, 2, 3].map(|col| matrix.at(row, col) as f32));
            }
        }
        rows
    }
}

/// A vertex of a skinned mesh at its pose, as the shaders work it out:
/// each of its four joints' rows (from `base` in `palette`) applied to
/// the point, and without the move to the normal, weighed by the joint's
/// share in order. A row past the palette's end is the identity's.
pub(crate) fn skin_vertex(palette: &[Row], base: u32, joints: [u8; 4], weights: [u8; 4], p: [f32; 3], n: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    const IDENTITY: [Row; 3] = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
    let mut point = [0.0f32; 3];
    let mut normal = [0.0f32; 3];
    for k in 0..4 {
        let w = weights[k] as f32 / 255.0;
        let row = base as usize + 3 * joints[k] as usize;
        let rows = match palette.get(row..row + 3) {
            Some(rows) => [rows[0], rows[1], rows[2]],
            None => IDENTITY,
        };
        let moved = rows.map(|r| r[0] * p[0] + r[1] * p[1] + r[2] * p[2] + r[3]);
        let turned = rows.map(|r| r[0] * n[0] + r[1] * n[1] + r[2] * n[2]);
        point = [0, 1, 2].map(|c| point[c] + w * moved[c]);
        normal = [0, 1, 2].map(|c| normal[c] + w * turned[c]);
    }
    (point, normal)
}

/// What a channel of a clip moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Path {
    Translation,
    Rotation,
    Scale,
}

/// One node's keys of one path: `times` in seconds, ascending, and a
/// value for each — three numbers, or four for a rotation `(x, y, z, w)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    pub node: u32,
    pub path: Path,
    /// Each key held until the next, instead of blended toward it.
    pub step: bool,
    pub times: Rc<[f32]>,
    pub values: Rc<[f32]>,
}

impl Channel {
    /// The key before `at` and how far toward the next it is, `[0, 1)`.
    fn span(&self, at: f64) -> Option<(usize, f64)> {
        let times = &self.times;
        let last = times.len().checked_sub(1)?;
        if at <= times[0] as f64 {
            return Some((0, 0.0));
        }
        if at >= times[last] as f64 {
            return Some((last, 0.0));
        }
        let next = times.partition_point(|&time| time as f64 <= at).min(last);
        let key = next - 1;
        let (from, to) = (times[key] as f64, times[next] as f64);
        let share = if to > from && !self.step { (at - from) / (to - from) } else { 0.0 };
        Some((key, share))
    }

    fn value(&self, key: usize, width: usize) -> Option<&[f32]> {
        self.values.get(key * width..key * width + width)
    }

    fn vector(&self, at: f64) -> Vec3 {
        let Some((key, share)) = self.span(at) else { return Vec3::ZERO };
        let point = |key: usize| self.value(key, 3).map_or(Vec3::ZERO, |v| vec3(v[0] as f64, v[1] as f64, v[2] as f64));
        let from = point(key);
        if share == 0.0 {
            return from;
        }
        from + (point(key + 1) - from) * share
    }

    fn rotation(&self, at: f64) -> Quat {
        let Some((key, share)) = self.span(at) else { return Quat::IDENTITY };
        let turn = |key: usize| {
            self.value(key, 4).map_or(Quat::IDENTITY, |v| Quat { x: v[0] as f64, y: v[1] as f64, z: v[2] as f64, w: v[3] as f64 })
        };
        let from = turn(key);
        if share == 0.0 {
            return from.normalized();
        }
        // the short way: q and −q are one rotation
        let mut to = turn(key + 1);
        if from.dot(to) < 0.0 {
            to = Quat { x: -to.x, y: -to.y, z: -to.z, w: -to.w };
        }
        Quat {
            x: from.x + (to.x - from.x) * share,
            y: from.y + (to.y - from.y) * share,
            z: from.z + (to.z - from.z) * share,
            w: from.w + (to.w - from.w) * share,
        }
        .normalized()
    }
}

/// A motion of a skeleton's nodes. Cheap to clone and to share.
#[derive(Clone, Debug)]
pub struct Clip(Rc<ClipInner>);

#[derive(Debug)]
struct ClipInner {
    id: u64,
    name: Option<Rc<str>>,
    duration: f64,
    channels: Vec<Channel>,
}

impl PartialEq for Clip {
    fn eq(&self, other: &Clip) -> bool {
        self.0.id == other.0.id
    }
}

impl Clip {
    /// A clip of `channels`; it lasts until its last key. A channel whose
    /// keys do not climb, or whose values do not match its keys, is left
    /// out.
    pub fn new(name: Option<Rc<str>>, channels: Vec<Channel>) -> Clip {
        let channels: Vec<Channel> = channels
            .into_iter()
            .filter(|channel| {
                let width = if channel.path == Path::Rotation { 4 } else { 3 };
                !channel.times.is_empty()
                    && channel.values.len() == channel.times.len() * width
                    && channel.times.windows(2).all(|pair| pair[0] < pair[1])
                    && channel.times.iter().chain(channel.values.iter()).all(|value| value.is_finite())
            })
            .collect();
        let duration = channels.iter().filter_map(|channel| channel.times.last()).fold(0.0f64, |end, &time| end.max(time as f64));
        Clip(Rc::new(ClipInner { id: next_id(), name, duration, channels }))
    }

    pub fn id(&self) -> u64 {
        self.0.id
    }

    pub fn name(&self) -> Option<&str> {
        self.0.name.as_deref()
    }

    /// Seconds, from its start to its last key.
    pub fn duration(&self) -> f64 {
        self.0.duration
    }

    pub fn channels(&self) -> &[Channel] {
        &self.0.channels
    }
}

/// A clip as an entity plays it: since when, how fast, and whether it
/// starts over at its end.
#[derive(Clone, Debug, PartialEq)]
pub struct Play {
    pub clip: Clip,
    /// The engine clock's time the clip's start stands at.
    pub start: f64,
    /// Clip seconds a second.
    pub speed: f64,
    pub looped: bool,
}

impl Play {
    /// The clip's own time at the engine's `time`: wrapped when it loops,
    /// held at its ends when it does not.
    pub fn clip_time(&self, time: f64) -> f64 {
        let duration = self.clip.duration();
        let at = (time - self.start) * self.speed;
        if !at.is_finite() || duration <= 0.0 {
            return 0.0;
        }
        if self.looped { at.rem_euclid(duration) } else { at.clamp(0.0, duration) }
    }

    /// Whether the pose still moves at `time`: a loop always does, a clip
    /// played once until its end.
    pub fn moving(&self, time: f64) -> bool {
        if self.clip.duration() <= 0.0 || self.speed == 0.0 {
            return false;
        }
        self.looped || {
            let at = (time - self.start) * self.speed;
            (0.0..self.clip.duration()).contains(&at)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_bones() -> Skeleton {
        Skeleton::new(vec![
            SkeletonNode { name: Some("root".into()), parent: None, rest: Transform::IDENTITY },
            SkeletonNode { name: Some("arm".into()), parent: Some(0), rest: Transform::from_translation(vec3(0.0, 1.0, 0.0)) },
        ])
    }

    #[test]
    fn the_rest_pose_is_the_identity_palette() {
        let skeleton = two_bones();
        let pose = skeleton.pose(None, 0.0);
        // inverse binds that undo the rest pose: every vertex stays put
        let binds = pose.iter().map(|matrix| matrix.inverse().expect("invertible")).collect();
        let skin = Skin::new(skeleton.clone(), vec![0, 1], binds).expect("a skin");
        let palette = skin.palette(&pose);
        assert_eq!(palette.len(), 6);
        for joint in palette.chunks(3) {
            assert_eq!(joint, [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]);
        }
        assert_eq!(skeleton.node("arm"), Some(1));
    }

    #[test]
    fn linear_keys_meet_halfway_and_step_keys_hold() {
        let channel = |step: bool| Channel {
            node: 1,
            path: Path::Translation,
            step,
            times: Rc::from([0.0, 1.0]),
            values: Rc::from([0.0, 1.0, 0.0, 2.0, 1.0, 0.0]),
        };
        let clip = Clip::new(Some("reach".into()), vec![channel(false)]);
        assert_eq!(clip.duration(), 1.0);
        let play = Play { clip, start: 10.0, speed: 1.0, looped: false };
        let pose = two_bones().pose(Some(&play), 10.5);
        assert_eq!(pose[1].translation_part(), vec3(1.0, 1.0, 0.0), "halfway along x");
        let held = Play { clip: Clip::new(None, vec![channel(true)]), start: 10.0, speed: 1.0, looped: false };
        assert_eq!(two_bones().pose(Some(&held), 10.9)[1].translation_part(), vec3(0.0, 1.0, 0.0), "a step key holds");
        // once, then still; a loop wraps
        assert!(play.moving(10.5) && !play.moving(11.5));
        assert_eq!(play.clip_time(12.0), 1.0);
        let looped = Play { looped: true, ..play };
        assert!(looped.moving(12.0));
        assert!((looped.clip_time(12.25) - 0.25).abs() < 1e-12);
    }

    #[test]
    fn a_rotation_takes_the_short_way() {
        let quarter = Quat::from_axis_angle(Vec3::Z, std::f64::consts::FRAC_PI_2);
        // the second key is the same turn written the long way
        let far = Quat { x: -quarter.x, y: -quarter.y, z: -quarter.z, w: -quarter.w };
        let channel = Channel {
            node: 0,
            path: Path::Rotation,
            step: false,
            times: Rc::from([0.0, 1.0]),
            values: Rc::from([0.0, 0.0, 0.0, 1.0, far.x as f32, far.y as f32, far.z as f32, far.w as f32]),
        };
        let play = Play { clip: Clip::new(None, vec![channel]), start: 0.0, speed: 1.0, looped: false };
        let pose = two_bones().pose(Some(&play), 0.5);
        // an eighth of a turn, not three: the arm leans toward −x
        let arm = pose[1].transform_point(Vec3::ZERO);
        assert!((arm.x + std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-6 && (arm.y - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-6, "{arm:?}");
    }

    #[test]
    fn a_broken_channel_is_left_out_and_a_loop_of_parents_is_cut() {
        let bad = Channel { node: 0, path: Path::Scale, step: false, times: Rc::from([1.0, 0.5]), values: Rc::from([1.0; 6]) };
        assert!(Clip::new(None, vec![bad]).channels().is_empty());
        let skeleton = Skeleton::new(vec![
            SkeletonNode { name: None, parent: Some(1), rest: Transform::IDENTITY },
            SkeletonNode { name: None, parent: Some(0), rest: Transform::IDENTITY },
        ]);
        assert_eq!(skeleton.pose(None, 0.0).len(), 2, "a loop poses without hanging");
        assert!(Skin::new(skeleton, vec![0; MAX_JOINTS + 1], Vec::new()).is_none());
    }
}

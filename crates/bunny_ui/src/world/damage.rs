//! What changed between two pictures of one world in one target, so a
//! tier draws only that over the picture the target holds.
//!
//! Two frames whose scenes match ([`WorldFrame::scene`]: the eye, the
//! style, the lights, the still shadow) differ only where their covers
//! do: an instance whose key is in both frames shows the same pixels in
//! both. What changed is the rect of every cover found in one frame and
//! not the other, and, when the map of what moves changed, where the
//! shadows of what moves fall in each. A frame with a bloom, or a change
//! past half of the target, is drawn whole.
//!
//! A parallel eye at Low that only moved across the view
//! ([`super::frame::Pan`]) moves the whole picture by whole pixels: the
//! held picture slides by the same step, and the strips the slide
//! uncovers are drawn with what changed.

use std::collections::HashMap;

use super::frame::WorldFrame;

/// The most rects a redraw takes; more merge into fewer.
pub const MAX_RECTS: usize = 4;

/// How to draw a frame over the picture of the one before it in the same
/// target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reuse {
    /// How far the held picture slides, in pixels: x right, y down.
    pub shift: [i32; 2],
    /// What to draw again, in pixels `[x, y, width, height]` from the top
    /// left: what the slide uncovers and what changed. Empty when nothing
    /// did.
    pub rects: Vec<[u32; 4]>,
}

/// The rects of `next` to draw again over a target that holds `held`, in
/// pixels `[x, y, width, height]` from the top left: empty when nothing it
/// shows changed. `None` when it must be drawn whole — or slid
/// ([`reuse`]).
pub fn redraw_rects(held: &WorldFrame, next: &WorldFrame) -> Option<Vec<[u32; 4]>> {
    reuse(held, next).filter(|reuse| reuse.shift == [0, 0]).map(|reuse| reuse.rects)
}

/// How to draw `next` over a target that holds `held`: slid by a step,
/// then some rects drawn again. `None` when it must be drawn whole.
pub fn reuse(held: &WorldFrame, next: &WorldFrame) -> Option<Reuse> {
    if held.size != next.size || held.samples != next.samples || held.bloom > 0 || next.bloom > 0 {
        return None;
    }
    let (width, height) = (next.size.0 as i64, next.size.1 as i64);
    let shift = if held.scene == next.scene {
        [0, 0]
    } else {
        // the eye moved across the view, and nothing else of the scene
        let (Some(was), Some(is)) = (held.pan, next.pan) else { return None };
        let reach = (was.depth[1] - was.depth[0]).abs().max(1e-9);
        let same_depth = (0..2).all(|k| (was.depth[k] - is.depth[k]).abs() <= reach * 1e-9);
        if was.key != is.key || !same_depth {
            return None;
        }
        let step = [is.origin[0] - was.origin[0], is.origin[1] - was.origin[1]];
        if step[0].abs() >= width || step[1].abs() >= height {
            return None;
        }
        [step[0] as i32, step[1] as i32]
    };
    let (dx, dy) = (shift[0] as f32, shift[1] as f32);
    let mut changed: Vec<[f32; 4]> = Vec::new();
    // what the slide uncovers
    let (w, h) = (width as f32, height as f32);
    if dx > 0.0 {
        changed.push([0.0, 0.0, dx, h]);
    } else if dx < 0.0 {
        changed.push([w + dx, 0.0, w, h]);
    }
    if dy > 0.0 {
        changed.push([0.0, 0.0, w, dy]);
    } else if dy < 0.0 {
        changed.push([0.0, h + dy, w, h]);
    }
    // a held rect, where the slide leaves it
    let slid = |rect: [f32; 4]| [rect[0] + dx, rect[1] + dy, rect[2] + dx, rect[3] + dy];
    // each key as many times as it shows: the covers one frame has more
    // of than the other changed
    let mut counts = HashMap::<u64, i64>::new();
    for cover in &held.covers {
        *counts.entry(cover.key).or_default() += 1;
    }
    for cover in &next.covers {
        *counts.entry(cover.key).or_default() -= 1;
    }
    let mut unmatched = |covers: &[super::frame::Cover], sign: i64| {
        for cover in covers {
            if let Some(count) = counts.get_mut(&cover.key)
                && *count * sign > 0
            {
                *count -= sign;
                changed.push(if sign > 0 { slid(cover.rect) } else { cover.rect });
            }
        }
    };
    unmatched(&held.covers, 1);
    unmatched(&next.covers, -1);
    // the map of what moves changed: where its shadows fell, and fall
    let moving = |frame: &WorldFrame| frame.moving_shadow.as_ref().map(|shadow| shadow.generation);
    if moving(held) != moving(next) {
        changed.extend(held.moving_covers.iter().map(|rect| slid(*rect)));
        changed.extend(&next.moving_covers);
    }
    let rects = merged(&changed, next.size);
    let area: u64 = rects.iter().map(|rect| rect[2] as u64 * rect[3] as u64).sum();
    if area * 2 > next.size.0 as u64 * next.size.1 as u64 {
        return None;
    }
    Some(Reuse { shift, rects })
}

/// Whole pixels around `rects` (`[left, top, right, bottom]`), inside a
/// target of `size`, as at most [`MAX_RECTS`] rects: two that meet become
/// their hull when it costs no more than both (an L of two strips stays
/// two), and while too many are left, the two whose hull adds the least.
/// Rects may overlap: a pixel drawn again in a second rect is drawn the
/// same.
fn merged(rects: &[[f32; 4]], size: (u32, u32)) -> Vec<[u32; 4]> {
    let mut boxes: Vec<[u32; 4]> = rects
        .iter()
        .map(|r| {
            let clamp = |value: f32, end: u32| (value.max(0.0) as u32).min(end);
            [clamp(r[0].floor(), size.0), clamp(r[1].floor(), size.1), clamp(r[2].ceil(), size.0), clamp(r[3].ceil(), size.1)]
        })
        .filter(|b| b[0] < b[2] && b[1] < b[3])
        .collect();
    // a crowd is one rect: the pairs below grow with the square
    if boxes.len() > 64 {
        let hull = boxes.iter().fold(boxes[0], |a, b| hull(a, *b));
        boxes = vec![hull];
    }
    let area = |b: [u32; 4]| (b[2] - b[0]) as u64 * (b[3] - b[1]) as u64;
    loop {
        let meeting = (0..boxes.len()).flat_map(|i| (i + 1..boxes.len()).map(move |j| (i, j))).find(|&(i, j)| {
            let (a, b) = (boxes[i], boxes[j]);
            a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3] && area(hull(a, b)) <= area(a) + area(b)
        });
        let pair = meeting.or_else(|| {
            (boxes.len() > MAX_RECTS)
                .then(|| {
                    (0..boxes.len())
                        .flat_map(|i| (i + 1..boxes.len()).map(move |j| (i, j)))
                        .min_by_key(|&(i, j)| area(hull(boxes[i], boxes[j])) - area(boxes[i]) - area(boxes[j]).min(area(hull(boxes[i], boxes[j])) - area(boxes[i])))
                })
                .flatten()
        });
        let Some((i, j)) = pair else { break };
        let joined = hull(boxes[i], boxes[j]);
        boxes.swap_remove(j);
        boxes[i] = joined;
    }
    boxes.sort_unstable_by_key(|b| (b[1], b[0]));
    boxes.into_iter().map(|b| [b[0], b[1], b[2] - b[0], b[3] - b[1]]).collect()
}

fn hull(a: [u32; 4], b: [u32; 4]) -> [u32; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Color;
    use crate::world::camera::Camera;
    use crate::world::frame::{Background, Quality, WorldStyle, build_frame};
    use crate::world::material::{Light, LineStyle, Material, PartState};
    use crate::world::math::{Vec3, vec3};
    use crate::world::mesh::{Lines, Mesh};
    use crate::world::soft;
    use crate::world::store::{Entity, World};

    const SIZE: (u32, u32) = (160, 120);

    fn style(quality: Quality) -> WorldStyle {
        WorldStyle { background: Background::Color(Color::rgb(236, 233, 226)), quality, ..WorldStyle::default() }
    }

    /// A floor, a box and a ball under a casting sun, and a figure that
    /// will move.
    fn room() -> (World, Entity) {
        let world = World::new();
        world.add(Mesh::plane(8.0, 8.0), Material::color(Color::rgb(200, 196, 188))).set_position(vec3(0.0, -0.5, 0.0));
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::rgb(222, 120, 60)));
        world.add(Mesh::sphere(0.4), Material::color(Color::rgb(70, 110, 200))).set_position(vec3(2.0, -0.1, -1.5));
        let figure = world.add(Mesh::cuboid(0.4, 1.2, 0.4), Material::color(Color::rgb(60, 140, 90))).set_position(vec3(-2.0, 0.1, 1.5));
        world.add_light(Light::sun().toward(vec3(1.0, -1.6, 0.7)).with_shadows());
        (world, figure)
    }

    fn frame(world: &World, quality: Quality, time: f64) -> WorldFrame {
        let camera = Camera::orbiting(Vec3::ZERO, 30.0, 35.0, 9.0);
        world.with(|data| build_frame(data, &camera, &style(quality), SIZE, 1.0, time))
    }

    /// `next` drawn over `held` in the rects between them is `next` drawn
    /// whole, byte for byte; the rects, which must be a part.
    fn proven(held: &WorldFrame, next: &WorldFrame) -> Vec<[u32; 4]> {
        let rects = redraw_rects(held, next).expect("a part of the picture");
        let over = soft::draw_over_with(&soft::draw(held), next, &rects, &|_| None);
        let whole = soft::draw(next);
        let wrong = over.rgba.chunks(4).zip(whole.rgba.chunks(4)).filter(|(a, b)| a != b).count();
        assert_eq!(wrong, 0, "the redraw in {rects:?} misses {wrong} pixels");
        rects
    }

    #[test]
    fn a_figure_that_walks_redraws_where_it_was_and_is_and_where_its_shadow_falls() {
        let (world, figure) = room();
        figure.move_to(vec3(-0.5, 0.1, 2.0), 1.0);
        let (before, after) = (frame(&world, Quality::Medium, 0.2), frame(&world, Quality::Medium, 0.5));
        assert!(after.moving_shadow.is_some() && !after.moving_covers.is_empty(), "it casts into the map of what moves");
        let rects = proven(&before, &after);
        let area: u32 = rects.iter().map(|r| r[2] * r[3]).sum();
        assert!(area > 0 && area * 4 < SIZE.0 * SIZE.1, "a small part: {rects:?}");
        // at Low it casts nothing: only where it stands
        let (before, after) = (frame(&world, Quality::Low, 0.2), frame(&world, Quality::Low, 0.5));
        proven(&before, &after);
    }

    #[test]
    fn a_new_look_redraws_only_its_own_pixels() {
        let (world, figure) = room();
        let before = frame(&world, Quality::Medium, 0.0);
        figure.set_tint(Color::rgb(250, 30, 30));
        proven(&before, &frame(&world, Quality::Medium, 0.0));
        let before = frame(&world, Quality::Medium, 0.0);
        figure.set_parts(Some(vec![PartState { tint: Color::rgb(20, 20, 200), shown: true }].into()));
        proven(&before, &frame(&world, Quality::Medium, 0.0));
    }

    #[test]
    fn a_pose_or_a_morph_redraws_its_figure_and_its_shadow() {
        use crate::world::material::Model;
        use crate::world::math::Transform;
        use crate::world::mesh::Morph;
        use crate::world::skin::{Channel, Clip, Path, Skeleton, SkeletonNode, Skin};
        use std::rc::Rc;
        let (world, _) = room();
        let skeleton = Skeleton::new(vec![
            SkeletonNode { name: Some("root".into()), parent: None, rest: Transform::IDENTITY },
            SkeletonNode { name: Some("arm".into()), parent: Some(0), rest: Transform::IDENTITY },
        ]);
        let skin = Skin::new(skeleton, vec![0, 1], Vec::new()).expect("a skin");
        let mut data = Mesh::cuboid(0.15, 0.5, 0.15).data().clone();
        data.joints = vec![[0, 1, 0, 0]; data.positions.len()];
        data.weights = data.positions.iter().map(|p| if p[1] > 0.0 { [0, 255, 0, 0] } else { [255, 0, 0, 0] }).collect();
        data.morphs = vec![Morph { positions: data.positions.iter().map(|p| [0.0, 0.0, p[1].max(0.0) * 0.5]).collect(), normals: Vec::new() }];
        let figure = world.add_model(Model::single(Mesh::new(data), Material::color(Color::rgb(220, 90, 40))).skinned(skin));
        figure.set_position(vec3(1.6, -0.25, 1.2));
        let sway = Channel { node: 1, path: Path::Translation, step: false, times: Rc::from([0.0, 1.0]), values: Rc::from([0.0, 0.0, 0.0, 0.2, 0.0, 0.0]) };
        figure.play(&Clip::new(Some("sway".into()), vec![sway]));
        let (before, after) = (frame(&world, Quality::High, 0.25), frame(&world, Quality::High, 0.5));
        assert_ne!(before.joints, after.joints, "the pose moved");
        proven(&before, &after);
        // a morph weight on a figure that rests
        let still = world.add_model(Model::single(figure_mesh(), Material::color(Color::rgb(90, 90, 200))));
        still.set_position(vec3(-1.5, 0.0, -1.0));
        let before = frame(&world, Quality::Medium, 3.0);
        still.set_morph([0.8, 0.0]);
        let rects = redraw_rects(&before, &frame(&world, Quality::Medium, 3.0));
        // it rests, so it casts into the still map, which changes: whole
        assert_eq!(rects, None);
    }

    fn figure_mesh() -> Mesh {
        use crate::world::mesh::Morph;
        let mut data = Mesh::cuboid(0.3, 1.0, 0.3).data().clone();
        data.morphs = vec![Morph { positions: data.positions.iter().map(|p| [p[0], 0.0, 0.0]).collect(), normals: Vec::new() }];
        Mesh::new(data)
    }

    #[test]
    fn what_comes_and_goes_redraws_its_place() {
        let (world, _) = room();
        let before = frame(&world, Quality::Medium, 0.0);
        let added = world.add(Mesh::sphere(0.3), Material::color(Color::rgba(240, 200, 40, 128)).blend()).set_position(vec3(0.5, 0.2, 1.4));
        let after = frame(&world, Quality::Medium, 0.0);
        proven(&before, &after);
        added.remove();
        proven(&after, &frame(&world, Quality::Medium, 0.0));
    }

    #[test]
    fn an_edge_that_moves_redraws_its_width() {
        let world = World::new();
        world.add(Mesh::plane(8.0, 8.0), Material::color(Color::rgb(200, 196, 188))).set_position(vec3(0.0, -0.5, 0.0));
        let square = [vec3(0.0, 0.0, 0.0), vec3(1.0, 0.0, 0.0), vec3(1.0, 0.0, 1.0), vec3(0.0, 0.0, 1.0)];
        let edges = world.add_lines(Lines::path(&square, true), LineStyle::solid(Color::rgb(30, 30, 30), 3.0));
        let before = frame(&world, Quality::Low, 0.0);
        edges.set_position(vec3(0.7, 0.0, 0.2));
        proven(&before, &frame(&world, Quality::Low, 0.0));
    }

    #[test]
    fn nothing_seen_changed_is_no_rect() {
        let (world, _) = room();
        let (before, after) = (frame(&world, Quality::Medium, 0.0), frame(&world, Quality::Medium, 3.0));
        assert_eq!(redraw_rects(&before, &after), Some(Vec::new()), "the clock alone moves no pixel");
    }

    #[test]
    fn a_new_eye_style_or_still_shadow_draws_whole() {
        let (world, figure) = room();
        let before = frame(&world, Quality::Medium, 0.0);
        let camera = Camera::orbiting(Vec3::ZERO, 31.0, 35.0, 9.0);
        let turned = world.with(|data| build_frame(data, &camera, &style(Quality::Medium), SIZE, 1.0, 0.0));
        assert_eq!(redraw_rects(&before, &turned), None, "the eye turned");
        let exposed = world.with(|data| {
            let style = WorldStyle { exposure: 1.2, ..style(Quality::Medium) };
            build_frame(data, &Camera::orbiting(Vec3::ZERO, 30.0, 35.0, 9.0), &style, SIZE, 1.0, 0.0)
        });
        assert_eq!(redraw_rects(&before, &exposed), None, "the style changed");
        // placed, not walked: it stays in the still map, which changes
        figure.set_position(vec3(-1.0, 0.1, 1.5));
        assert_eq!(redraw_rects(&before, &frame(&world, Quality::Medium, 0.0)), None, "the still shadow moved");
    }

    /// The room through a parallel eye at Low over `target`, the eye
    /// `height` meters across the view.
    fn iso(world: &World, target: Vec3, time: f64) -> WorldFrame {
        let mut camera = Camera::orbiting(target, 45.0, 35.0, 20.0);
        camera.lens = crate::world::camera::Lens::Parallel { height: 9.0 };
        world.with(|data| build_frame(data, &camera, &style(Quality::Low), SIZE, 1.0, time))
    }

    #[test]
    fn a_parallel_eye_at_low_stands_on_the_pixel_grid() {
        let (world, _) = room();
        let still = iso(&world, Vec3::ZERO, 0.0);
        let pan = still.pan.expect("a parallel eye at Low");
        // the origin of the world sits mid-target, on a pixel's corner
        assert!((pan.origin[0] - SIZE.0 as i64 / 2).abs() <= 1 && (pan.origin[1] - SIZE.1 as i64 / 2).abs() <= 1, "{pan:?}");
        // a pan by a fraction of a pixel stands on the grid too: the
        // origin moves by whole pixels, and the dither with it
        let moved = iso(&world, vec3(0.013, 0.0, -0.021), 0.0);
        let phase = |frame: &WorldFrame| frame.uniforms.tone[2] as i64 - 1;
        let expect = |origin: [i64; 2]| (-origin[0]).rem_euclid(4) + 4 * (-origin[1]).rem_euclid(4);
        assert_eq!(phase(&still), expect(pan.origin));
        assert_eq!(phase(&moved), expect(moved.pan.expect("a pan").origin));
        // not at Medium, nor through a perspective eye
        assert!(frame_at_medium(&world).pan.is_none());
        assert!(frame(&world, Quality::Low, 0.0).pan.is_none());
    }

    fn frame_at_medium(world: &World) -> WorldFrame {
        let mut camera = Camera::orbiting(Vec3::ZERO, 45.0, 35.0, 20.0);
        camera.lens = crate::world::camera::Lens::Parallel { height: 9.0 };
        world.with(|data| build_frame(data, &camera, &style(Quality::Medium), SIZE, 1.0, 0.0))
    }

    /// `next` drawn over `held` slid and redrawn in its rects is `next`
    /// drawn whole, byte for byte: the eye stands on the pixel grid and the
    /// dither runs with the world. The slide, which must be one.
    fn slid_exactly(held: &WorldFrame, next: &WorldFrame) -> Reuse {
        let reuse = reuse(held, next).expect("a slide");
        assert_ne!(reuse.shift, [0, 0], "the eye moved");
        let over = soft::draw_over_with(&soft::slid(&soft::draw(held), reuse.shift), next, &reuse.rects, &|_| None);
        let whole = soft::draw(next);
        let wrong = over.rgba.chunks(4).zip(whole.rgba.chunks(4)).filter(|(a, b)| a != b).count();
        assert_eq!(wrong, 0, "the slide by {:?} in {:?} misses {wrong} pixels", reuse.shift, reuse.rects);
        reuse
    }

    #[test]
    fn a_pan_at_low_slides_the_picture_and_draws_what_it_uncovers() {
        let (world, _) = room();
        let before = iso(&world, Vec3::ZERO, 0.0);
        let after = iso(&world, vec3(0.4, 0.0, -0.25), 0.0);
        let reuse = slid_exactly(&before, &after);
        // the two strips the slide uncovers, and nothing else
        assert_eq!(reuse.rects.len(), 2, "{reuse:?}");
        let area: u32 = reuse.rects.iter().map(|r| r[2] * r[3]).sum();
        let uncovered = reuse.shift[0].unsigned_abs() * SIZE.1 + reuse.shift[1].unsigned_abs() * SIZE.0;
        assert_eq!(area, uncovered, "{reuse:?}");
    }

    #[test]
    fn a_pan_while_a_figure_walks_slides_and_redraws_the_walk() {
        let (world, figure) = room();
        figure.move_to(vec3(-0.5, 0.1, 2.0), 1.0);
        let before = iso(&world, Vec3::ZERO, 0.2);
        let after = iso(&world, vec3(-0.3, 0.0, 0.2), 0.4);
        slid_exactly(&before, &after);
    }

    #[test]
    fn a_pan_by_an_awkward_step_still_slides() {
        // an eye and a target moved by one step, as a hand's pan moves
        // them: the turn between them may change in its last bit
        let (world, _) = room();
        let looking = |shift: Vec3| {
            let start = Camera::orbiting(vec3(0.1, 0.0, 0.7), 45.0, 35.0, 20.0);
            let mut camera = Camera::looking_at(start.eye + shift, start.target + shift);
            camera.lens = crate::world::camera::Lens::Parallel { height: 9.0 };
            world.with(|data| build_frame(data, &camera, &style(Quality::Low), SIZE, 1.0, 0.0))
        };
        let before = looking(Vec3::ZERO);
        // steps whose turn differs from the first in its last bit
        let turn = |shift: Vec3| {
            let start = Camera::orbiting(vec3(0.1, 0.0, 0.7), 45.0, 35.0, 20.0);
            let view = Camera::looking_at(start.eye + shift, start.target + shift).view();
            [0, 1, 2, 4, 5, 6, 8, 9, 10].map(|k| view.m[k].to_bits())
        };
        for step in [vec3(2.116_573_886_622_883, 0.0, 0.775_746_603_011_303_3), vec3(2.575_375_156_993_571_2, 0.0, -2.105_203_682_180_405_4)] {
            assert_ne!(turn(step), turn(Vec3::ZERO), "the turn moved by a last bit");
            slid_exactly(&before, &looking(step));
        }
    }

    #[test]
    fn a_turn_or_a_zoom_does_not_slide() {
        let (world, _) = room();
        let before = iso(&world, Vec3::ZERO, 0.0);
        let mut turned = Camera::orbiting(Vec3::ZERO, 46.0, 35.0, 20.0);
        turned.lens = crate::world::camera::Lens::Parallel { height: 9.0 };
        let turned = world.with(|data| build_frame(data, &turned, &style(Quality::Low), SIZE, 1.0, 0.0));
        assert_eq!(reuse(&before, &turned), None);
        let mut zoomed = Camera::orbiting(Vec3::ZERO, 45.0, 35.0, 20.0);
        zoomed.lens = crate::world::camera::Lens::Parallel { height: 8.0 };
        let zoomed = world.with(|data| build_frame(data, &zoomed, &style(Quality::Low), SIZE, 1.0, 0.0));
        assert_eq!(reuse(&before, &zoomed), None);
    }

    #[test]
    fn many_far_apart_changes_merge_into_few_rects() {
        let rects: Vec<[f32; 4]> = (0..9).map(|k| [k as f32 * 15.0, 5.0, k as f32 * 15.0 + 4.0, 9.0]).collect();
        let merged = merged(&rects, (200, 100));
        assert!(merged.len() <= MAX_RECTS, "{merged:?}");
        for rect in &rects {
            let inside = merged.iter().any(|m| rect[0] >= m[0] as f32 && rect[2] <= (m[0] + m[2]) as f32 && rect[1] >= m[1] as f32 && rect[3] <= (m[1] + m[3]) as f32);
            assert!(inside, "{rect:?} is redrawn");
        }
        // two that mostly overlap are one; an L of two strips stays two
        assert_eq!(merged_of(&[[0.0, 0.0, 10.0, 10.0], [2.0, 1.0, 11.0, 10.0]]), vec![[0, 0, 11, 10]]);
        assert_eq!(merged_of(&[[96.0, 0.0, 100.0, 100.0], [0.0, 98.0, 100.0, 100.0]]).len(), 2);
    }

    fn merged_of(rects: &[[f32; 4]]) -> Vec<[u32; 4]> {
        merged(rects, (100, 100))
    }
}

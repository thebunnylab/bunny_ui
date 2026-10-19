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

use std::collections::HashMap;

use super::frame::WorldFrame;

/// The most rects a redraw takes; more merge into fewer.
pub const MAX_RECTS: usize = 4;

/// The rects of `next` to draw again over a target that holds `held`, in
/// pixels `[x, y, width, height]` from the top left: empty when nothing it
/// shows changed. `None` when it must be drawn whole.
pub fn redraw_rects(held: &WorldFrame, next: &WorldFrame) -> Option<Vec<[u32; 4]>> {
    if held.size != next.size || held.samples != next.samples || held.scene != next.scene || held.bloom > 0 || next.bloom > 0 {
        return None;
    }
    let mut changed: Vec<[f32; 4]> = Vec::new();
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
                changed.push(cover.rect);
            }
        }
    };
    unmatched(&held.covers, 1);
    unmatched(&next.covers, -1);
    // the map of what moves changed: where its shadows fell, and fall
    let moving = |frame: &WorldFrame| frame.moving_shadow.as_ref().map(|shadow| shadow.generation);
    if moving(held) != moving(next) {
        changed.extend(&held.moving_covers);
        changed.extend(&next.moving_covers);
    }
    let rects = merged(&changed, next.size);
    let area: u64 = rects.iter().map(|rect| rect[2] as u64 * rect[3] as u64).sum();
    if area * 2 > next.size.0 as u64 * next.size.1 as u64 {
        return None;
    }
    Some(rects)
}

/// Whole pixels around `rects` (`[left, top, right, bottom]`), inside a
/// target of `size`, as at most [`MAX_RECTS`] rects that never overlap:
/// rects that meet become their hull, and while too many are left, the
/// two whose hull adds the least.
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
        let meeting = (0..boxes.len())
            .flat_map(|i| (i + 1..boxes.len()).map(move |j| (i, j)))
            .find(|&(i, j)| boxes[i][0] < boxes[j][2] && boxes[j][0] < boxes[i][2] && boxes[i][1] < boxes[j][3] && boxes[j][1] < boxes[i][3]);
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

    #[test]
    fn many_far_apart_changes_merge_into_few_rects() {
        let rects: Vec<[f32; 4]> = (0..9).map(|k| [k as f32 * 15.0, 5.0, k as f32 * 15.0 + 4.0, 9.0]).collect();
        let merged = merged(&rects, (200, 100));
        assert!(merged.len() <= MAX_RECTS, "{merged:?}");
        for rect in &rects {
            let inside = merged.iter().any(|m| rect[0] >= m[0] as f32 && rect[2] <= (m[0] + m[2]) as f32 && rect[1] >= m[1] as f32 && rect[3] <= (m[1] + m[3]) as f32);
            assert!(inside, "{rect:?} is redrawn");
        }
        // two that overlap are one
        assert_eq!(merged_of(&[[0.0, 0.0, 10.0, 10.0], [5.5, 5.0, 20.0, 12.0]]), vec![[0, 0, 20, 12]]);
    }

    fn merged_of(rects: &[[f32; 4]]) -> Vec<[u32; 4]> {
        merged(rects, (100, 100))
    }
}

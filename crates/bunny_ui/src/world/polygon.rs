//! Polygons in a plane: an outline with holes, cut into triangles.
//!
//! The method is ear clipping. Each hole joins the outline through a
//! bridge — two edges to a vertex the hole can see — and the result is
//! one ring that walks around the holes. Then the ring loses one convex
//! corner at a time, each corner a triangle with no other vertex of the
//! ring inside it.
//!
//! The cost is quadratic in the number of vertices: right for a room, a
//! floor plan or the face of a model, and the place to optimize when a
//! polygon of many thousand corners arrives. A polygon that crosses
//! itself or collapses still gives triangles — never a panic, never a
//! loop that does not end.

use super::math::Vec2;

/// Twice the signed area of a ring: positive when it turns
/// counter-clockwise (x right, y up).
pub fn signed_area(points: &[Vec2]) -> f64 {
    let n = points.len();
    (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum::<f64>()
        * 0.5
}

/// Triangles that cover the polygon `outline` minus its `holes`.
///
/// The indices count the points in order: the outline first, then each
/// hole in turn. Each triangle turns counter-clockwise (x right, y up),
/// whatever the turn of the rings given. Points repeated back to back
/// are skipped.
pub fn triangulate(outline: &[Vec2], holes: &[&[Vec2]]) -> Vec<[u32; 3]> {
    let mut points: Vec<Vec2> = Vec::with_capacity(outline.len() + holes.iter().map(|h| h.len()).sum::<usize>());
    points.extend_from_slice(outline);
    let mut ring = ring_of(&points, 0, outline.len(), true);
    if ring.len() < 3 {
        return Vec::new();
    }
    // the hole that reaches farthest right joins first: its bridge can
    // never cross a hole that has not joined yet on that side
    let mut pending: Vec<Vec<usize>> = Vec::new();
    for hole in holes {
        let start = points.len();
        points.extend_from_slice(hole);
        let hole_ring = ring_of(&points, start, hole.len(), false);
        if hole_ring.len() >= 3 {
            pending.push(hole_ring);
        }
    }
    pending.sort_by(|a, b| {
        let reach = |ring: &Vec<usize>| ring.iter().map(|&i| points[i].x).fold(f64::NEG_INFINITY, f64::max);
        reach(b).total_cmp(&reach(a))
    });
    for index in 0..pending.len() {
        let (joined, rest) = pending.split_at(index + 1);
        ring = bridge(&points, ring, &joined[index], rest);
    }
    clip_ears(&points, ring)
}

/// The ring of `count` points from `start`, without back-to-back
/// repeats, turned counter-clockwise for an outline and clockwise for a
/// hole — the turn that keeps the inside of the polygon on the left of
/// every edge.
fn ring_of(points: &[Vec2], start: usize, count: usize, outline: bool) -> Vec<usize> {
    let mut ring: Vec<usize> = Vec::with_capacity(count);
    for index in start..start + count {
        if !points[index].x.is_finite() || !points[index].y.is_finite() {
            continue;
        }
        if ring.last().is_some_and(|&last| points[last] == points[index]) {
            continue;
        }
        ring.push(index);
    }
    while ring.len() > 1 && points[ring[0]] == points[*ring.last().unwrap()] {
        ring.pop();
    }
    let ring_points: Vec<Vec2> = ring.iter().map(|&i| points[i]).collect();
    let area = signed_area(&ring_points);
    if (outline && area < 0.0) || (!outline && area > 0.0) {
        ring.reverse();
    }
    ring
}

/// Twice the signed area of the triangle `a b c`: positive for a left
/// turn.
fn turn(a: Vec2, b: Vec2, c: Vec2) -> f64 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

/// Do the closed segments `a b` and `c d` cross or touch?
fn segments_meet(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> bool {
    let (d1, d2) = (turn(c, d, a), turn(c, d, b));
    let (d3, d4) = (turn(a, b, c), turn(a, b, d));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0)) {
        return true;
    }
    let on = |p: Vec2, q: Vec2, r: Vec2| {
        r.x >= p.x.min(q.x) && r.x <= p.x.max(q.x) && r.y >= p.y.min(q.y) && r.y <= p.y.max(q.y)
    };
    (d1 == 0.0 && on(c, d, a)) || (d2 == 0.0 && on(c, d, b)) || (d3 == 0.0 && on(a, b, c)) || (d4 == 0.0 && on(a, b, d))
}

/// Is `target` inside the corner at `corner` of a ring with the inside
/// on the left — the test that a bridge leaves the vertex INTO the
/// polygon?
fn opens_toward(prev: Vec2, corner: Vec2, next: Vec2, target: Vec2) -> bool {
    if turn(prev, corner, next) >= 0.0 {
        // a convex corner: inside both edges
        turn(prev, corner, target) > 0.0 && turn(corner, next, target) > 0.0
    } else {
        // a reflex corner: inside either edge
        turn(prev, corner, target) > 0.0 || turn(corner, next, target) > 0.0
    }
}

/// Joins `hole` into `ring` through the nearest vertex both can see. The
/// two bridge vertices appear twice in the result, once on each side of
/// the cut.
fn bridge(points: &[Vec2], ring: Vec<usize>, hole: &[usize], later: &[Vec<usize>]) -> Vec<usize> {
    // the hole's rightmost vertex: nothing of the hole lies right of it
    let (h_at, &h) = hole
        .iter()
        .enumerate()
        .max_by(|a, b| points[*a.1].x.total_cmp(&points[*b.1].x).then(points[*b.1].y.total_cmp(&points[*a.1].y)))
        .expect("a hole ring has three vertices");
    let hp = points[h];
    let h_prev = points[hole[(h_at + hole.len() - 1) % hole.len()]];
    let h_next = points[hole[(h_at + 1) % hole.len()]];

    let edges_of = |r: &[usize]| -> Vec<(Vec2, Vec2)> {
        (0..r.len()).map(|i| (points[r[i]], points[r[(i + 1) % r.len()]])).collect()
    };
    let mut walls = edges_of(&ring);
    walls.extend(edges_of(hole));
    for other in later {
        walls.extend(edges_of(other));
    }

    let mut best: Option<(f64, usize)> = None;
    for (at, &v) in ring.iter().enumerate() {
        let vp = points[v];
        let distance = (vp - hp).length();
        if best.is_some_and(|(d, _)| d <= distance) {
            continue;
        }
        let prev = points[ring[(at + ring.len() - 1) % ring.len()]];
        let next = points[ring[(at + 1) % ring.len()]];
        if vp != hp && !(opens_toward(prev, vp, next, hp) && opens_toward(h_prev, hp, h_next, vp)) {
            continue;
        }
        let blocked = walls.iter().any(|&(a, b)| {
            // an edge that ends at either end of the bridge does not block it
            if a == vp || b == vp || a == hp || b == hp {
                return false;
            }
            segments_meet(vp, hp, a, b)
        });
        if !blocked {
            best = Some((distance, at));
        }
    }
    // no vertex sees the hole (a hole outside the outline, or one that
    // crosses it): the nearest vertex joins anyway, and the ears decide
    let at = best.map(|(_, at)| at).unwrap_or_else(|| {
        (0..ring.len())
            .min_by(|&a, &b| (points[ring[a]] - hp).length().total_cmp(&(points[ring[b]] - hp).length()))
            .expect("a ring has vertices")
    });

    let mut joined = Vec::with_capacity(ring.len() + hole.len() + 2);
    joined.extend_from_slice(&ring[..=at]);
    joined.extend(hole[h_at..].iter().chain(hole[..h_at].iter()));
    joined.push(h);
    joined.push(ring[at]);
    joined.extend_from_slice(&ring[at + 1..]);
    joined
}

/// Cuts a counter-clockwise ring into triangles, one ear at a time.
fn clip_ears(points: &[Vec2], ring: Vec<usize>) -> Vec<[u32; 3]> {
    let mut triangles = Vec::with_capacity(ring.len().saturating_sub(2));
    let mut ring = ring;
    let mut cursor = 0;
    let mut misses = 0;
    while ring.len() > 3 {
        let n = ring.len();
        let (i0, i1, i2) = ((cursor + n - 1) % n, cursor % n, (cursor + 1) % n);
        let (a, b, c) = (points[ring[i0]], points[ring[i1]], points[ring[i2]]);
        let area = turn(a, b, c);
        if area == 0.0 && misses >= n {
            // a stalled ring: a collinear corner adds no area, and losing it ends the stall
            ring.remove(i1);
            cursor = i0 % ring.len();
            misses = 0;
            continue;
        }
        if area > 0.0 && is_ear(points, &ring, a, b, c) {
            triangles.push([ring[i0] as u32, ring[i1] as u32, ring[i2] as u32]);
            ring.remove(i1);
            cursor = i0 % ring.len();
            misses = 0;
            continue;
        }
        misses += 1;
        if misses > 2 * n {
            // no ear anywhere: the ring crosses itself. Cut the corner
            // with the largest left turn so the loop always ends.
            let pick = (0..n)
                .max_by(|&x, &y| {
                    let at = |k: usize| turn(points[ring[(k + n - 1) % n]], points[ring[k]], points[ring[(k + 1) % n]]);
                    at(x).total_cmp(&at(y))
                })
                .unwrap_or(0);
            let (p0, p2) = ((pick + n - 1) % n, (pick + 1) % n);
            if turn(points[ring[p0]], points[ring[pick]], points[ring[p2]]) > 0.0 {
                triangles.push([ring[p0] as u32, ring[pick] as u32, ring[p2] as u32]);
            }
            ring.remove(pick);
            cursor = pick % ring.len();
            misses = 0;
            continue;
        }
        cursor = (cursor + 1) % n;
    }
    if ring.len() == 3 && turn(points[ring[0]], points[ring[1]], points[ring[2]]) > 0.0 {
        triangles.push([ring[0] as u32, ring[1] as u32, ring[2] as u32]);
    }
    triangles
}

/// No other vertex of the ring lies in the triangle — on its edges
/// counts as in, so an ear never swallows a vertex that touches it. A
/// vertex at the same place as a corner (the two ends of a bridge) does
/// not count.
fn is_ear(points: &[Vec2], ring: &[usize], a: Vec2, b: Vec2, c: Vec2) -> bool {
    let (min_x, max_x) = (a.x.min(b.x).min(c.x), a.x.max(b.x).max(c.x));
    let (min_y, max_y) = (a.y.min(b.y).min(c.y), a.y.max(b.y).max(c.y));
    ring.iter().all(|&index| {
        let p = points[index];
        if p == a || p == b || p == c || p.x < min_x || p.x > max_x || p.y < min_y || p.y > max_y {
            return true;
        }
        !(turn(a, b, p) >= 0.0 && turn(b, c, p) >= 0.0 && turn(c, a, p) >= 0.0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::math::vec2;

    fn area_of(points: &[Vec2], triangles: &[[u32; 3]]) -> f64 {
        triangles
            .iter()
            .map(|t| turn(points[t[0] as usize], points[t[1] as usize], points[t[2] as usize]) * 0.5)
            .sum()
    }

    fn all_points(outline: &[Vec2], holes: &[&[Vec2]]) -> Vec<Vec2> {
        let mut points = outline.to_vec();
        for hole in holes {
            points.extend_from_slice(hole);
        }
        points
    }

    fn covers(points: &[Vec2], triangles: &[[u32; 3]], p: Vec2) -> bool {
        triangles.iter().any(|t| {
            let (a, b, c) = (points[t[0] as usize], points[t[1] as usize], points[t[2] as usize]);
            turn(a, b, p) > 0.0 && turn(b, c, p) > 0.0 && turn(c, a, p) > 0.0
        })
    }

    fn square(x: f64, y: f64, size: f64) -> Vec<Vec2> {
        vec![vec2(x, y), vec2(x + size, y), vec2(x + size, y + size), vec2(x, y + size)]
    }

    #[test]
    fn a_square_with_a_hole_keeps_its_area() {
        let outline = square(0.0, 0.0, 10.0);
        let hole = square(3.0, 3.0, 2.0);
        let triangles = triangulate(&outline, &[&hole]);
        let points = all_points(&outline, &[&hole]);
        assert!((area_of(&points, &triangles) - 96.0).abs() < 1e-9, "{}", area_of(&points, &triangles));
        assert!(triangles.iter().all(|t| {
            turn(points[t[0] as usize], points[t[1] as usize], points[t[2] as usize]) > 0.0
        }), "every triangle turns left");
    }

    #[test]
    fn the_triangulation_never_covers_a_hole() {
        let outline = square(0.0, 0.0, 12.0);
        let left = square(2.0, 2.0, 3.0);
        let right = square(7.0, 6.0, 3.0);
        let triangles = triangulate(&outline, &[&left, &right]);
        let points = all_points(&outline, &[&left, &right]);
        assert!((area_of(&points, &triangles) - (144.0 - 18.0)).abs() < 1e-9);
        for hole in [&left, &right] {
            for i in 1..10 {
                for j in 1..10 {
                    let p = vec2(hole[0].x + 0.3 * i as f64, hole[0].y + 0.3 * j as f64);
                    assert!(!covers(&points, &triangles, p), "{p:?} lies in a hole");
                }
            }
        }
        // and the solid part is covered (off the diagonals a bridge may take)
        assert!(covers(&points, &triangles, vec2(1.0, 1.7)));
        assert!(covers(&points, &triangles, vec2(6.1, 2.9)));
    }

    #[test]
    fn a_clockwise_outline_triangulates_like_a_counter_clockwise_one() {
        // an L, given both ways round
        let l = vec![
            vec2(0.0, 0.0),
            vec2(4.0, 0.0),
            vec2(4.0, 1.0),
            vec2(1.0, 1.0),
            vec2(1.0, 3.0),
            vec2(0.0, 3.0),
        ];
        let mut reversed = l.clone();
        reversed.reverse();
        for outline in [&l, &reversed] {
            let triangles = triangulate(outline, &[]);
            assert_eq!(triangles.len(), 4);
            assert!((area_of(outline, &triangles) - 6.0).abs() < 1e-9);
            assert!(!covers(outline, &triangles, vec2(2.5, 2.0)), "the notch of the L stays open");
        }
    }

    #[test]
    fn degenerate_polygons_never_panic() {
        let cases: Vec<Vec<Vec2>> = vec![
            vec![],
            vec![vec2(0.0, 0.0)],
            vec![vec2(0.0, 0.0), vec2(1.0, 0.0)],
            vec![vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(2.0, 0.0)],
            vec![vec2(0.0, 0.0), vec2(0.0, 0.0), vec2(0.0, 0.0), vec2(0.0, 0.0)],
            vec![vec2(0.0, 0.0), vec2(2.0, 2.0), vec2(2.0, 0.0), vec2(0.0, 2.0)], // a bow tie
            vec![vec2(f64::NAN, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(1.0, 1.0)],
            vec![vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 0.0)],
        ];
        for outline in &cases {
            let triangles = triangulate(outline, &[]);
            assert!(triangles.iter().flatten().all(|&i| (i as usize) < outline.len()));
        }
        // a hole outside the outline, and a hole bigger than the outline
        let outline = square(0.0, 0.0, 1.0);
        let far = square(5.0, 5.0, 1.0);
        let big = square(-1.0, -1.0, 4.0);
        let _ = triangulate(&outline, &[&far]);
        let _ = triangulate(&outline, &[&big]);
        let _ = triangulate(&outline, &[&outline]);
    }

    #[test]
    fn a_hole_touching_the_outline_still_triangulates() {
        let outline = square(0.0, 0.0, 6.0);
        // a hole with a corner on the outline's left edge
        let hole = vec![vec2(0.0, 3.0), vec2(2.0, 2.0), vec2(2.0, 4.0)];
        let triangles = triangulate(&outline, &[&hole]);
        let points = all_points(&outline, &[&hole]);
        assert!((area_of(&points, &triangles) - (36.0 - 2.0)).abs() < 1e-9, "{}", area_of(&points, &triangles));
        assert!(!covers(&points, &triangles, vec2(1.5, 3.0)));
    }
}

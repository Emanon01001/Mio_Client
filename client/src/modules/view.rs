//! World -> screen projection shared by the modules that draw in the world
//! (esp, waypoints, chunk borders). See analysis/notes/esp.md for the maths.

use super::game::Camera;

/// Ray entry distance for a normalized direction. Parallel handling avoids
/// division by zero/NaNs on faces; invalid and zero directions are rejected.
pub(crate) fn ray_box(
    origin: [f32; 3],
    dir: [f32; 3],
    bounds: super::game::Aabb,
    reach: f32,
) -> Option<f32> {
    let length = dot(dir, dir);
    if !origin.iter().chain(bounds.iter()).all(|x| x.is_finite())
        || !dir.iter().all(|x| x.is_finite())
        || !length.is_finite()
        || length < 1e-12
        || !(reach.is_finite() && reach >= 0.)
        || (0..3).any(|i| bounds[i] >= bounds[i + 3])
    {
        return None;
    }
    let (mut near, mut far) = (0_f32, reach);
    for axis in 0..3 {
        if dir[axis].abs() < 1e-8 {
            if origin[axis] < bounds[axis] || origin[axis] > bounds[axis + 3] {
                return None;
            }
        } else {
            let a = (bounds[axis] - origin[axis]) / dir[axis];
            let b = (bounds[axis + 3] - origin[axis]) / dir[axis];
            near = near.max(a.min(b));
            far = far.min(a.max(b));
            if near > far {
                return None;
            }
        }
    }
    Some(near)
}

/// Points closer to the camera plane than this are not projected.
const NEAR: f32 = 0.1;

pub type ScreenSegment = ([f32; 2], [f32; 2]);
/// The 12 edges of a box, as pairs of corner indices (bit 0 = x, 1 = y, 2 = z).
pub(super) const BOX_EDGES: [(usize, usize); 12] = [
    (0, 1),
    (2, 3),
    (4, 5),
    (6, 7),
    (0, 2),
    (1, 3),
    (4, 6),
    (5, 7),
    (0, 4),
    (1, 5),
    (2, 6),
    (3, 7),
];

/// A box ready to draw.
pub enum BoxLines {
    /// Every corner projects within the clipping bounds (the usual case): the
    /// bottom and top faces as closed loops, corner `i` of one above corner
    /// `i` of the other (the four vertical edges).
    Loops {
        bottom: [[f32; 2]; 4],
        top: [[f32; 2]; 4],
    },
    /// A box crossing the camera plane or reaching far off screen: its edges
    /// clipped one by one.
    Segments([Option<ScreenSegment>; 12]),
}

/// Corners of the bottom (y = min) and top faces, in loop order; corner `i`
/// has bit 0 for x, bit 1 for y and bit 2 for z at max.
const BOTTOM_LOOP: [usize; 4] = [0, 1, 5, 4];
const TOP_LOOP: [usize; 4] = [2, 3, 7, 6];

/// Draws boxes with ImGui's own primitives on the background draw list: a
/// box of [`BoxLines::Loops`] takes six calls (two closed loops and four
/// edges) instead of twelve, the same pixels apart from the loops' joined
/// corners.
pub struct BoxPainter {
    list: *mut hudhook::imgui::sys::ImDrawList,
}
impl BoxPainter {
    /// The background draw list of the frame being built. Use only within
    /// the current ImGui frame (the `Ui` is the proof of one).
    pub fn new(_ui: &hudhook::imgui::Ui) -> Self {
        Self {
            list: unsafe { hudhook::imgui::sys::igGetBackgroundDrawList() },
        }
    }
    /// Draws `lines` and returns whether anything was drawn.
    pub fn draw(&self, lines: &BoxLines, color: [f32; 4], thickness: f32) -> bool {
        use hudhook::imgui::sys;
        let color = hudhook::imgui::ImColor32::from(color).to_bits();
        // ImGui's AddLine strokes through pixel centres; match it.
        let at = |p: [f32; 2]| sys::ImVec2 {
            x: p[0] + 0.5,
            y: p[1] + 0.5,
        };
        let line = |a: [f32; 2], b: [f32; 2]| unsafe {
            sys::ImDrawList_AddLine(
                self.list,
                sys::ImVec2 { x: a[0], y: a[1] },
                sys::ImVec2 { x: b[0], y: b[1] },
                color,
                thickness,
            );
        };
        match lines {
            BoxLines::Loops { bottom, top } => {
                for face in [bottom, top] {
                    let points = face.map(at);
                    unsafe {
                        sys::ImDrawList_AddPolyline(
                            self.list,
                            points.as_ptr(),
                            points.len() as i32,
                            color,
                            sys::ImDrawFlags_Closed as i32,
                            thickness,
                        );
                    }
                }
                for (&a, &b) in bottom.iter().zip(top) {
                    line(a, b);
                }
                true
            }
            BoxLines::Segments(segments) => {
                let mut any = false;
                for &(a, b) in segments.iter().flatten() {
                    line(a, b);
                    any = true;
                }
                any
            }
        }
    }
}

/// Project each box corner once, then clip its edges. No heap allocation.
/// The early plane tests use the same margin as `clip`, so boxes crossing
/// the camera plane or surrounding the screen are still retained.
#[cfg(test)]
pub fn box_segments(
    camera: &Camera,
    size: [f32; 2],
    min: [f32; 3],
    max: [f32; 3],
) -> [Option<ScreenSegment>; 12] {
    match box_lines(camera, size, min, max) {
        None => [None; 12],
        Some(BoxLines::Segments(segments)) => segments,
        Some(BoxLines::Loops { bottom, top }) => {
            // Back to the edge order of BOX_EDGES.
            let mut corner = [[0.; 2]; 8];
            for i in 0..4 {
                corner[BOTTOM_LOOP[i]] = bottom[i];
                corner[TOP_LOOP[i]] = top[i];
            }
            BOX_EDGES.map(|(a, b)| Some((corner[a], corner[b])))
        }
    }
}

/// Project each box corner once. No heap allocation. `None`: nothing of the
/// box is on screen.
pub fn box_lines(
    camera: &Camera,
    size: [f32; 2],
    min: [f32; 3],
    max: [f32; 3],
) -> Option<BoxLines> {
    let points: [[f32; 3]; 8] = std::array::from_fn(|i| {
        let point = std::array::from_fn(|k| if i >> k & 1 == 0 { min[k] } else { max[k] });
        let rel = sub(point, camera.origin);
        [
            dot(rel, camera.right) * camera.scale[0],
            dot(rel, camera.up) * camera.scale[1],
            -dot(rel, camera.back),
        ]
    });
    if points.iter().all(|p| p[2] < NEAR)
        || (0..2).any(|axis| {
            points.iter().all(|p| p[axis] < -3. * p[2])
                || points.iter().all(|p| p[axis] > 3. * p[2])
        })
    {
        return None;
    }
    let pixels = |p: [f32; 3]| {
        [
            size[0] * 0.5 * (1. + p[0] / p[2]),
            size[1] * 0.5 * (1. - p[1] / p[2]),
        ]
    };
    let projected = points.map(|p| (p[2] >= NEAR).then(|| pixels(p)));
    // All corners within `clip`'s bounds: no edge needs clipping.
    let inside = |p: [f32; 2]| {
        (-size[0]..=size[0] * 2.).contains(&p[0]) && (-size[1]..=size[1] * 2.).contains(&p[1])
    };
    if projected.iter().all(|p| p.is_some_and(inside)) {
        let corner = |i: usize| projected[i].unwrap();
        return Some(BoxLines::Loops {
            bottom: BOTTOM_LOOP.map(corner),
            top: TOP_LOOP.map(corner),
        });
    }
    Some(BoxLines::Segments(BOX_EDGES.map(|(a, b)| {
        match (projected[a], projected[b]) {
            (Some(a), Some(b)) => clip(size, a, b),
            (None, None) => None,
            (pa, pb) => {
                let (front, back, front_pixels, reversed) = if let Some(pa) = pa {
                    (points[a], points[b], pa, false)
                } else {
                    (points[b], points[a], pb.unwrap(), true)
                };
                let t = (NEAR * 1.01 - back[2]) / (front[2] - back[2]);
                let cut = pixels(std::array::from_fn(|k| back[k] + (front[k] - back[k]) * t));
                if reversed {
                    clip(size, cut, front_pixels)
                } else {
                    clip(size, front_pixels, cut)
                }
            }
        }
    })))
}

pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Distance in front of the camera plane (negative: behind the camera).
fn depth(camera: &Camera, point: [f32; 3]) -> f32 {
    -dot(sub(point, camera.origin), camera.back)
}

/// World position -> screen pixels, or None if behind the camera.
pub fn project(camera: &Camera, size: [f32; 2], point: [f32; 3]) -> Option<[f32; 2]> {
    let rel = sub(point, camera.origin);
    let depth = -dot(rel, camera.back);
    if depth < NEAR {
        return None;
    }
    let x = dot(rel, camera.right) / depth * camera.scale[0];
    let y = dot(rel, camera.up) / depth * camera.scale[1];
    Some([size[0] * 0.5 * (1.0 + x), size[1] * 0.5 * (1.0 - y)])
}

/// A world-space line segment -> screen pixels. Unlike projecting the two ends
/// separately, a segment that crosses the camera plane is cut there instead of
/// being dropped; None only if it lies entirely behind the camera.
pub fn segment(
    camera: &Camera,
    size: [f32; 2],
    mut a: [f32; 3],
    mut b: [f32; 3],
) -> Option<([f32; 2], [f32; 2])> {
    let (da, db) = (depth(camera, a), depth(camera, b));
    if da < NEAR && db < NEAR {
        return None;
    }
    // Move the end that is behind the plane onto it (plus a hair, so the
    // rounding of the interpolation cannot leave it behind).
    let cut = |from: [f32; 3], to: [f32; 3], d_from: f32, d_to: f32| {
        let t = (NEAR * 1.01 - d_from) / (d_to - d_from);
        [
            from[0] + (to[0] - from[0]) * t,
            from[1] + (to[1] - from[1]) * t,
            from[2] + (to[2] - from[2]) * t,
        ]
    };
    if da < NEAR {
        a = cut(a, b, da, db);
    } else if db < NEAR {
        b = cut(b, a, db, da);
    }
    clip(size, project(camera, size, a)?, project(camera, size, b)?)
}

/// Cuts a screen-space segment to the screen plus one screen of margin on each
/// side (Liang-Barsky); None if nothing of it is left. A point just in front of
/// the camera projects to enormous coordinates, which this keeps away from the
/// renderer.
fn clip(size: [f32; 2], a: [f32; 2], b: [f32; 2]) -> Option<([f32; 2], [f32; 2])> {
    let (min, max) = ([-size[0], -size[1]], [size[0] * 2.0, size[1] * 2.0]);
    let delta = [b[0] - a[0], b[1] - a[1]];
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for axis in 0..2 {
        // Entering / leaving parameters for the two edges on this axis.
        for (p, q) in [
            (-delta[axis], a[axis] - min[axis]),
            (delta[axis], max[axis] - a[axis]),
        ] {
            if p == 0.0 {
                if q < 0.0 {
                    return None;
                }
            } else if p < 0.0 {
                t0 = t0.max(q / p);
            } else {
                t1 = t1.min(q / p);
            }
        }
    }
    if t0.is_nan() || t1.is_nan() || t0 > t1 {
        return None;
    }
    let at = |t: f32| [a[0] + delta[0] * t, a[1] + delta[1] * t];
    Some((at(t0), at(t1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// At the origin, looking down -Z (so `back` is +Z), 90 degree FOV.
    fn camera() -> Camera {
        Camera {
            origin: [0.0, 0.0, 0.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            back: [0.0, 0.0, 1.0],
            scale: [1.0, 1.0],
        }
    }

    const SIZE: [f32; 2] = [200.0, 100.0];

    #[test]
    fn projects_centre_edges_and_rejects_points_behind() {
        let c = camera();
        assert_eq!(project(&c, SIZE, [0.0, 0.0, -10.0]), Some([100.0, 50.0]));
        // 45 degrees right / up lands on the screen edge at 90 degree FOV.
        assert_eq!(project(&c, SIZE, [10.0, 0.0, -10.0]), Some([200.0, 50.0]));
        assert_eq!(project(&c, SIZE, [0.0, 10.0, -10.0]), Some([100.0, 0.0]));
        assert_eq!(project(&c, SIZE, [0.0, 0.0, 10.0]), None);
    }

    #[test]
    fn segment_crossing_the_camera_plane_is_cut_not_dropped() {
        let c = camera();
        // Straight ahead, from 10 in front to 10 behind: stays on the centre.
        let (a, b) = segment(&c, SIZE, [0.0, 0.0, -10.0], [0.0, 0.0, 10.0]).unwrap();
        assert_eq!(a, [100.0, 50.0]);
        assert!((b[0] - 100.0).abs() < 0.01 && (b[1] - 50.0).abs() < 0.01);
        // Same segment given the other way round.
        assert!(segment(&c, SIZE, [0.0, 0.0, 10.0], [0.0, 0.0, -10.0]).is_some());
        // Entirely behind.
        assert!(segment(&c, SIZE, [0.0, 0.0, 5.0], [3.0, 0.0, 10.0]).is_none());
        // Entirely in front: identical to projecting both ends.
        let both = segment(&c, SIZE, [1.0, 0.0, -5.0], [0.0, 2.0, -8.0]).unwrap();
        assert_eq!(Some(both.0), project(&c, SIZE, [1.0, 0.0, -5.0]));
        assert_eq!(Some(both.1), project(&c, SIZE, [0.0, 2.0, -8.0]));
    }

    #[test]
    fn clip_bounds_coordinates_and_drops_offscreen_segments() {
        // Inside: untouched.
        assert_eq!(
            clip(SIZE, [10.0, 10.0], [150.0, 90.0]),
            Some(([10.0, 10.0], [150.0, 90.0]))
        );
        // A horizontal line far wider than the screen is cut to the margin.
        let (a, b) = clip(SIZE, [-1.0e6, 50.0], [1.0e6, 50.0]).unwrap();
        assert!((a[0] + 200.0).abs() < 1.0 && (b[0] - 400.0).abs() < 1.0);
        assert_eq!((a[1], b[1]), (50.0, 50.0));
        // Entirely outside, both as a diagonal and parallel to an edge.
        assert_eq!(clip(SIZE, [900.0, 900.0], [5000.0, 950.0]), None);
        assert_eq!(clip(SIZE, [0.0, -500.0], [200.0, -500.0]), None);
        // A pillar right next to the camera stays within the margin.
        let c = camera();
        let (a, b) = segment(&c, SIZE, [0.5, -100.0, -0.2], [0.5, 100.0, -0.2]).unwrap();
        for p in [a, b] {
            // Within the margin, give or take float rounding on the long line.
            assert!(p[1] >= -101.0 && p[1] <= 201.0, "{p:?}");
        }
        assert!(
            b[1] - a[1] < -250.0,
            "the visible part spans the screen: {a:?} {b:?}"
        );
    }

    #[test]
    fn shared_corner_projection_preserves_clipped_edges() {
        let cameras = [
            camera(),
            Camera {
                origin: [103.25, 117.62, -18.25],
                right: [0.8, 0., -0.6],
                up: [0., 1., 0.],
                back: [0.6, 0., 0.8],
                scale: [0.7, 1.3],
            },
        ];
        for c in cameras {
            for z in [-10., -1., -0.25, 0., 0.5, 10.] {
                for x in [-100., -5., 0., 5., 100.] {
                    for extent in [1., 20.] {
                        let min = [c.origin[0] + x, c.origin[1] - extent * 0.5, c.origin[2] + z];
                        let max = [min[0] + extent, min[1] + extent, min[2] + extent];
                        let corners: [[f32; 3]; 8] = std::array::from_fn(|i| {
                            std::array::from_fn(|k| if i >> k & 1 == 0 { min[k] } else { max[k] })
                        });
                        let fast = box_segments(&c, SIZE, min, max);
                        for (i, (a, b)) in BOX_EDGES.into_iter().enumerate() {
                            let expected = segment(&c, SIZE, corners[a], corners[b]);
                            match (fast[i], expected) {
                                (None, None) => (),
                                (Some((a, b)), Some((ea, eb))) => {
                                    for (p, e) in [a, b]
                                        .into_iter()
                                        .flatten()
                                        .zip([ea, eb].into_iter().flatten())
                                    {
                                        assert!((p - e).abs() < 0.1, "{p} vs {e}; {min:?}");
                                    }
                                }
                                other => panic!("edge {i}: {other:?}; {min:?}"),
                            }
                        }
                    }
                }
            }
        }
    }

    /// Old (twelve `add_line`) against new (`BoxPainter`) block outlines:
    /// a 16x16x16 grid of blocks ahead, like BlockESP at its 4,096 limit.
    /// `MCBE_UI_PREVIEW` exports each way's frame for a visual comparison.
    #[test]
    #[ignore = "manual: BlockESP outline cost; MCBE_UI_PREVIEW exports frames"]
    fn block_outline_paint_cost() {
        let size = [1280f32, 720f32];
        let camera = Camera {
            origin: [8.3, 20.6, 30.4],
            right: [0.8, 0., -0.6],
            up: [0., 1., 0.],
            back: [0.6, 0.18, 0.78],
            scale: [1. / 0.8, 1. / 0.45],
        };
        let mut ctx = hudhook::imgui::Context::create();
        ctx.set_ini_filename(None);
        let texture = ctx.fonts().build_alpha8_texture();
        let atlas = ([texture.width, texture.height], texture.data.to_vec());
        ctx.io_mut().display_size = size;
        ctx.io_mut().delta_time = 1. / 60.;
        // Over 65,535 vertices: the game's renderer supports vertex offsets.
        ctx.io_mut().backend_flags |= hudhook::imgui::BackendFlags::RENDERER_HAS_VTX_OFFSET;
        let blocks: Vec<[f32; 3]> = (0..4096)
            .map(|i| [(i % 16) as f32, (i / 16 % 16) as f32, (i / 256) as f32])
            .collect();
        for way in ["old", "new"] {
            let mut times = Vec::new();
            let mut vertices = 0;
            for frame in 0..220 {
                let ui = ctx.frame();
                let start = std::time::Instant::now();
                let color = [0.4, 0.85, 1., 1.];
                if way == "old" {
                    let draw = ui.get_background_draw_list();
                    for &p in &blocks {
                        for (a, b) in box_segments(&camera, size, p, p.map(|v| v + 1.))
                            .into_iter()
                            .flatten()
                        {
                            draw.add_line(a, b, color).thickness(1.5).build();
                        }
                    }
                } else {
                    let painter = BoxPainter::new(ui);
                    for &p in &blocks {
                        if let Some(lines) = box_lines(&camera, size, p, p.map(|v| v + 1.)) {
                            painter.draw(&lines, color, 1.5);
                        }
                    }
                }
                let elapsed = start.elapsed();
                let data = ctx.render();
                vertices = data.total_vtx_count;
                if frame >= 20 {
                    times.push(elapsed);
                }
                if frame == 219 {
                    if let Some(dir) = std::env::var_os("MCBE_UI_PREVIEW") {
                        crate::gui::preview::export_frame(
                            &std::path::Path::new(&dir).join(way),
                            data,
                            &atlas,
                        );
                    }
                }
            }
            times.sort();
            println!(
                "BlockESP 4096 blocks ({way}): median {:?}, p90 {:?}, {vertices} vertices",
                times[100], times[180]
            );
        }
    }
}

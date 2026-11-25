//! Ray/box intersection over immutable snapshots, with block occlusion.
use super::{actors::Actor, blocks::LookRay, Target};
#[cfg(test)]
use crate::game::Aabb;
use std::collections::HashMap;

fn direction(v: [f32; 3]) -> Option<[f32; 3]> {
    let length = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    (length.is_finite() && length > 1e-6).then(|| v.map(|x| x / length))
}
use crate::modules::view::ray_box as intersection;

pub(crate) fn pick(
    ray: &LookRay,
    actors: Option<&HashMap<u32, Actor>>,
    reach: f32,
) -> Option<Target> {
    let dir = direction(ray.forward)?;
    if !ray.origin.iter().all(|v| v.is_finite() && v.abs() < 3e7)
        || !(reach.is_finite() && reach >= 0.)
        || !(ray.reach.is_finite() && ray.reach >= 0.)
    {
        return None;
    }
    let reach = reach.min(ray.reach);
    let block = ray
        .block
        .as_ref()
        .filter(|b| b.distance.is_finite() && b.distance >= 0. && b.distance <= reach);
    let mut nearest: Option<(u32, &Actor, f32)> = None;
    for (&id, actor) in actors.into_iter().flat_map(|m| m.iter()) {
        if actor.local {
            continue;
        }
        let Some(bounds) = actor.bounds else {
            continue;
        };
        let Some(distance) = intersection(ray.origin, dir, bounds, reach) else {
            continue;
        };
        // A tie belongs to the block: never expose an entity through its face.
        if block.is_some_and(|b| distance + 1e-4 >= b.distance) {
            continue;
        }
        if nearest.is_none_or(|(old_id, _, old_distance)| {
            distance
                .total_cmp(&old_distance)
                .then(id.cmp(&old_id))
                .is_lt()
        }) {
            nearest = Some((id, actor, distance));
        }
    }
    nearest
        .map(|(id, actor, distance)| Target::Entity {
            id,
            actor: actor.clone(),
            distance,
        })
        .or_else(|| block.cloned().map(Target::Block))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn actor(bounds: Aabb) -> Actor {
        Actor {
            name: "cow".into(),
            display_name: None,
            actor_type: 11,
            health: Some((8., 10.)),
            bounds: Some(bounds),
            local: false,
        }
    }
    fn ray() -> LookRay {
        LookRay {
            owner: (1, 2),
            origin: [0., 0.5, 0.],
            forward: [0., 0., 2.],
            reach: 6.,
            block: None,
        }
    }
    #[test]
    fn nearest_crosshair_hit_ignores_self_side_and_behind_and_respects_walls() {
        let mut r = ray();
        let mut a = HashMap::from([
            (7, actor([-0.5, 0., 2., 0.5, 1., 3.])),
            (4, actor([-0.5, 0., 4., 0.5, 1., 5.])),
            (1, actor([1., 0., 0., 2., 1., 1.])),
            (2, actor([-0.5, 0., -2., 0.5, 1., -1.])),
            (
                0,
                Actor {
                    local: true,
                    ..actor([-1., 0., -1., 1., 1., 1.])
                },
            ),
        ]);
        assert!(matches!(
            pick(&r, Some(&a), 6.),
            Some(Target::Entity {
                id: 7,
                distance: 2.,
                ..
            })
        ));
        r.block = Some(super::super::blocks::Look {
            name: "stone".into(),
            position: [0, 0, 1],
            distance: 1.,
        });
        assert!(matches!(pick(&r, Some(&a), 6.), Some(Target::Block(_))));
        r.block.as_mut().unwrap().distance = 2.;
        assert!(matches!(pick(&r, Some(&a), 6.), Some(Target::Block(_))));
        r.block.as_mut().unwrap().distance = 3.;
        assert!(matches!(
            pick(&r, Some(&a), 6.),
            Some(Target::Entity { id: 7, .. })
        ));
        r.block = None;
        assert!(pick(&r, Some(&a), 1.).is_none());
        a.insert(3, actor([-0.5, 0., 2., 0.5, 1., 3.]));
        assert!(matches!(
            pick(&r, Some(&a), 6.),
            Some(Target::Entity { id: 3, .. })
        ));
        assert!(pick(&r, None, 6.).is_none());
    }
    #[test]
    fn parallel_faces_inside_boxes_negative_direction_and_invalid_values() {
        let b = [0., 0., 0., 1., 1., 1.];
        for (o, d, expected) in [
            ([0., 0.5, -1.], [0., 0., 1.], Some(1.)),
            ([1., 0.5, 2.], [0., 0., -1.], Some(1.)),
            ([0.5; 3], [0., 1., 0.], Some(0.)),
            ([2., 0.5, -1.], [0., 0., 1.], None),
        ] {
            assert_eq!(intersection(o, d, b, 6.), expected);
        }
        let mut r = ray();
        let a = HashMap::from([(1, actor([-1., 0., 2., 1., 1., 3.]))]);
        for forward in [[0.; 3], [f32::NAN, 0., 1.], [f32::INFINITY, 0., 1.]] {
            r.forward = forward;
            assert!(pick(&r, Some(&a), 6.).is_none());
        }
        r = ray();
        r.origin[0] = f32::NAN;
        assert!(pick(&r, Some(&a), 6.).is_none());
        assert!(intersection([0.; 3], [0., 0., 1.], [0., 0., f32::NAN, 1., 1., 2.], 6.).is_none());
        assert!(intersection([0.; 3], [0., 0., 1.], [1., 0., 0., 0., 1., 1.], 6.).is_none());
        assert!(pick(&ray(), Some(&a), f32::NAN).is_none());
        r = ray();
        r.reach = 1.;
        assert!(pick(&r, Some(&a), 64.).is_none());
    }
}

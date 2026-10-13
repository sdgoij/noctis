//! Animation playback: sampling a clip's channels into joint transforms.
//!
//! An [`Animator`](crate::components::Animator) names a clip and a playhead;
//! [`sample_scene`] advances every animator and writes the sampled TRS into the
//! target joint entities' local transforms. The engine's `propagate_transforms`
//! then composes the joint world transforms the skinned draw reads, so animation
//! composes with the ECS instead of bypassing it (`.notes/animation.md`).

use crate::assets::{AnimationChannel, AnimationProperty, AssetRegistry};
use crate::components::{Animator, LocalTransform, Skeleton};
use crate::world::{Entity, World};
use math::{Quat, vec3};

/// Sample one channel at `time`, interpolating between its keys.
///
/// Before the first key the value is the first key's; after the last, the last's.
/// Translation and scale interpolate linearly, rotation by shortest-path
/// (normalized) lerp.
pub fn sample_channel(channel: &AnimationChannel, time: f32) -> Option<[f32; 4]> {
    let times = &channel.times;
    let count = times.len().min(channel.values.len());
    if count == 0 {
        return None;
    }
    if time <= times[0] {
        return Some(channel.values[0]);
    }
    if time >= times[count - 1] {
        return Some(channel.values[count - 1]);
    }

    // The segment `[i - 1, i]` containing `time`.
    let mut i = 1;
    while i < count && times[i] < time {
        i += 1;
    }
    let (t0, t1) = (times[i - 1], times[i]);
    let alpha = ((time - t0) / (t1 - t0).max(1e-6)).clamp(0.0, 1.0);
    let a = channel.values[i - 1];
    let b = channel.values[i];
    Some(match channel.property {
        AnimationProperty::Rotation => slerp(a, b, alpha),
        _ => lerp(a, b, alpha),
    })
}

/// Component-wise linear interpolation.
fn lerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

/// Shortest-path normalized lerp of two quaternions.
fn slerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let sign = if dot < 0.0 { -1.0 } else { 1.0 };
    let mut out = [
        a[0] + (sign * b[0] - a[0]) * t,
        a[1] + (sign * b[1] - a[1]) * t,
        a[2] + (sign * b[2] - a[2]) * t,
        a[3] + (sign * b[3] - a[3]) * t,
    ];
    let length = (out[0] * out[0] + out[1] * out[1] + out[2] * out[2] + out[3] * out[3])
        .sqrt()
        .max(1e-6);
    for value in &mut out {
        *value /= length;
    }
    out
}

/// Advance every [`Animator`] by `dt` and write the sampled pose into its
/// skeleton's joint transforms.
///
/// A looping clip wraps its playhead into `[0, duration)`; a one-shot holds on the
/// last frame and clears `playing`. Runs in the `update` stage, before
/// `propagate_transforms`, so the pose is composed this frame.
pub fn sample_scene(world: &mut World, assets: &AssetRegistry, dt: f32) {
    let animators: Vec<Entity> = world
        .query::<Animator>()
        .map(|(entity, _)| entity)
        .collect();
    for entity in animators {
        let Some(skeleton) = world.get::<Skeleton>(entity).cloned() else {
            continue;
        };
        let Some(animator) = world.get::<Animator>(entity).copied() else {
            continue;
        };
        let Some(clip) = assets.animations.get(animator.clip) else {
            continue;
        };

        let duration = clip.duration.max(0.0);
        let mut time = animator.time + dt * animator.speed;
        let mut playing = animator.playing;
        if duration > 0.0 {
            if animator.looped {
                time = time.rem_euclid(duration);
            } else if time >= duration {
                time = duration;
                playing = false;
            }
        }

        // Collect first: mutating a joint's transform cannot also hold the borrow
        // the sample pass needs.
        let pose: Vec<(Entity, AnimationProperty, [f32; 4])> = clip
            .channels
            .iter()
            .filter_map(|channel| {
                let value = sample_channel(channel, time)?;
                let target = *skeleton.0.get(channel.node)?;
                Some((target, channel.property, value))
            })
            .collect();

        for (target, property, value) in pose {
            if let Some(transform) = world.get_mut::<LocalTransform>(target) {
                match property {
                    AnimationProperty::Translation => {
                        transform.translation = vec3(value[0], value[1], value[2]);
                    }
                    AnimationProperty::Rotation => {
                        transform.rotation = Quat::new(value[0], value[1], value[2], value[3]);
                    }
                    AnimationProperty::Scale => {
                        transform.scale = vec3(value[0], value[1], value[2]);
                    }
                }
            }
        }

        if let Some(animator) = world.get_mut::<Animator>(entity) {
            animator.time = time;
            animator.playing = playing;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::{AnimationClip, AssetHandle};

    fn channel(node: usize, property: AnimationProperty) -> AnimationChannel {
        AnimationChannel {
            node,
            property,
            times: vec![0.0, 1.0],
            values: vec![[0.0, 0.0, 0.0, 0.0], [2.0, 0.0, 0.0, 0.0]],
        }
    }

    #[test]
    fn translation_interpolates_and_holds_past_the_ends() {
        let c = channel(0, AnimationProperty::Translation);
        assert_eq!(sample_channel(&c, 0.5), Some([1.0, 0.0, 0.0, 0.0]));
        assert_eq!(sample_channel(&c, -1.0), Some([0.0; 4]));
        assert_eq!(sample_channel(&c, 5.0), Some([2.0, 0.0, 0.0, 0.0]));
    }

    #[test]
    fn rotation_slerps_along_the_shortest_path() {
        let quarter = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let c = AnimationChannel {
            node: 0,
            property: AnimationProperty::Rotation,
            times: vec![0.0, 1.0],
            values: vec![
                [0.0, 0.0, 0.0, 1.0],
                [quarter.x, quarter.y, quarter.z, quarter.w],
            ],
        };
        let half = sample_channel(&c, 0.5).expect("a sample");
        let expected = Quat::from_rotation_y(std::f32::consts::FRAC_PI_4);
        assert!(
            (half[1] - expected.y).abs() < 1e-3,
            "half the turn: {half:?}"
        );
    }

    /// A world with one joint, an animator on a parent, and a clip that moves the
    /// joint up.
    fn rig(looped: bool) -> (World, AssetRegistry, Entity, Entity) {
        let mut assets = AssetRegistry::new();
        let clip = assets.animations.insert_named(
            "Move",
            AnimationClip {
                name: "Move".into(),
                duration: 1.0,
                channels: vec![channel(0, AnimationProperty::Translation)],
            },
        );

        let mut world = World::new();
        let joint = world.spawn();
        world.insert(joint, LocalTransform::identity());
        let root = world.spawn();
        world.insert(
            root,
            Animator {
                clip,
                looped,
                ..Animator::default()
            },
        );
        world.insert(root, Skeleton(vec![joint]));
        (world, assets, root, joint)
    }

    #[test]
    fn sampling_moves_a_joint_transform() {
        let (mut world, assets, _root, joint) = rig(true);
        sample_scene(&mut world, &assets, 0.5);
        let moved = world.get::<LocalTransform>(joint).expect("a transform");
        assert!((moved.translation.x - 1.0).abs() < 1e-4, "{moved:?}");
    }

    #[test]
    fn a_looping_clip_wraps_its_playhead() {
        let (mut world, assets, root, _joint) = rig(true);
        sample_scene(&mut world, &assets, 1.5);
        let animator = world.get::<Animator>(root).expect("an animator");
        assert!((animator.time - 0.5).abs() < 1e-4, "{animator:?}");
        assert!(animator.playing);
    }

    #[test]
    fn a_one_shot_holds_on_the_last_frame() {
        let (mut world, assets, root, _joint) = rig(false);
        sample_scene(&mut world, &assets, 5.0);
        let animator = world.get::<Animator>(root).expect("an animator");
        assert!((animator.time - 1.0).abs() < 1e-4, "{animator:?}");
        assert!(!animator.playing);
    }

    #[test]
    fn a_missing_clip_is_left_alone() {
        let mut world = World::new();
        let root = world.spawn();
        world.insert(root, Animator::new(AssetHandle::NONE));
        let joint = world.spawn();
        world.insert(root, Skeleton(vec![joint]));
        let assets = AssetRegistry::new();
        sample_scene(&mut world, &assets, 1.0);
        assert_eq!(world.get::<Animator>(root).map(|a| a.time), Some(0.0));
    }
}

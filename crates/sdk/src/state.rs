/// Blocks, blocks/second, or a unit direction, depending on the field.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
impl Vec3 {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
    pub fn length(self) -> f32 {
        self.x.hypot(self.y).hypot(self.z)
    }
    pub fn horizontal_length(self) -> f32 {
        self.x.hypot(self.z)
    }
    pub fn normalized(self) -> Option<Self> {
        let length = self.length();
        (length.is_finite() && length > 1e-6)
            .then(|| Self::new(self.x / length, self.y / length, self.z / length))
    }
}
impl From<[f32; 3]> for Vec3 {
    fn from([x, y, z]: [f32; 3]) -> Self {
        Self::new(x, y, z)
    }
}
impl From<Vec3> for [f32; 3] {
    fn from(v: Vec3) -> Self {
        [v.x, v.y, v.z]
    }
}

/// Minecraft yaw: 0=south (+Z), 90=west (-X). Pitch is positive down.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rotation {
    pub yaw: f32,
    pub pitch: f32,
}
impl Rotation {
    pub fn from_direction(direction: Vec3) -> Option<Self> {
        let d = direction.normalized()?;
        Some(Self {
            yaw: (-d.x).atan2(d.z).to_degrees(),
            pitch: (-d.y).clamp(-1.0, 1.0).asin().to_degrees(),
        })
    }
    pub fn direction(self) -> Vec3 {
        let (yaw, pitch) = (self.yaw.to_radians(), self.pitch.to_radians());
        Vec3::new(
            -yaw.sin() * pitch.cos(),
            -pitch.sin(),
            yaw.cos() * pitch.cos(),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Player {
    pub position: Vec3,
    pub eye_position: Vec3,
    pub velocity: Vec3,
    /// Current view/camera direction, not a target entity's rotation.
    pub rotation: Rotation,
}
impl Player {
    pub fn speed(self) -> f32 {
        self.velocity.length()
    }
    pub fn horizontal_speed(self) -> f32 {
        self.velocity.horizontal_length()
    }
    pub fn direction(self) -> Vec3 {
        self.rotation.direction()
    }
    pub(crate) fn is_finite(self) -> bool {
        self.position.is_finite()
            && self.eye_position.is_finite()
            && self.velocity.is_finite()
            && self.rotation.yaw.is_finite()
            && self.rotation.pitch.is_finite()
    }
}

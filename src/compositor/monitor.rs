use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Monitor {
    pub id: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
    #[serde(default = "default_scale")]
    pub scale: f64,
    #[serde(rename = "refreshRate")]
    pub refresh_rate: f64,
    pub focused: bool,
}

fn default_scale() -> f64 {
    1.0
}

impl Monitor {
    pub fn logical_width(&self) -> u32 {
        if self.scale > 0.0 {
            (self.width as f64 / self.scale).round() as u32
        } else {
            self.width
        }
    }

    pub fn logical_height(&self) -> u32 {
        if self.scale > 0.0 {
            (self.height as f64 / self.scale).round() as u32
        } else {
            self.height
        }
    }
}

//! An agent's avatar (bots-avatar.html; `bot-avatar.tsx`): two SVG layers from `design/icons/bot/<shape>`, the body
//! tinted the agent's colour and the eyes white. The eyes are the cropped layer, placed over their box, so a pose
//! scales them about their own centre (GPUI transforms an SVG about the centre of its bounds). A pose is a still look;
//! `motion::eyes` gives a living avatar its gaze each frame (blinks, the scan, glances). Sizes are in rems, so the
//! avatar follows the app zoom.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, IntoElement, ParentElement as _, RenderOnce, Rgba, Styled as _, Transformation, Window,
    div, point, px, size, svg,
};
use protocol::model::{BotColor, BotShape};

use crate::ui::style::{hsla, palette, r, theme};

/// The eyes' state (bots-status.html): each agent state has its look, so a still frame still reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pose {
    Idle,
    /// Needs you: the eyes look up at you.
    Waiting,
    /// Focused: the lids narrow.
    Working,
    /// Drowsy: the lids half close and the eyes drop.
    Limited,
    /// Resting: the eyes all but closed.
    Paused,
}

/// Where the eyes are: the lids' scale (1 open, 0.08 shut) and the look's offset, in units of the 40-unit avatar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gaze {
    pub lid: f32,
    pub x: f32,
    pub y: f32,
}

impl Gaze {
    /// A pose's still look (bots-status.html).
    pub fn of(pose: Pose) -> Gaze {
        let (lid, y) = match pose {
            Pose::Idle => (1.0, 0.0),
            Pose::Waiting => (1.0, -1.7),
            Pose::Working => (0.7, 0.0),
            Pose::Limited => (0.4, 1.0),
            Pose::Paused => (0.16, 0.0),
        };
        Gaze { lid, x: 0., y }
    }
}

#[derive(IntoElement)]
pub struct BotAvatar {
    color: BotColor,
    shape: BotShape,
    size: f32,
    pose: Pose,
    gaze: Option<Gaze>,
}

impl BotAvatar {
    /// `size` in design px: 20, 28, 40 or 64 in the Aegis set.
    pub fn new(color: BotColor, shape: BotShape, size: f32) -> Self {
        BotAvatar {
            color,
            shape,
            size,
            pose: Pose::Idle,
            gaze: None,
        }
    }

    pub fn pose(mut self, pose: Pose) -> Self {
        self.pose = pose;
        self
    }

    /// This frame's gaze, for a living avatar; otherwise the pose's still look.
    pub fn gaze(mut self, gaze: Gaze) -> Self {
        self.gaze = Some(gaze);
        self
    }
}

impl RenderOnce for BotAvatar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let folder = format!("icons/bot/{}", shape_name(self.shape));
        let unit = self.size / 40.0;
        // The eyes' offset is a transformation, in pixels: design px times the zoom's rem.
        let zoom = f32::from(window.rem_size()) / 16.0;
        let eyes = eye_box(self.shape);
        let gaze = self.gaze.unwrap_or(Gaze::of(self.pose));
        div()
            .relative()
            .flex_none()
            .size(r(self.size))
            .child(
                svg()
                    .path(format!("{folder}/body.svg"))
                    .size_full()
                    .text_color(hsla(bot_color(self.color, cx))),
            )
            .when_some(eyes, |avatar, [x, y, width, height]| {
                avatar.child(
                    svg()
                        .path(format!("{folder}/eyes-crop.svg"))
                        .absolute()
                        .left(r(x * unit))
                        .top(r(y * unit))
                        .w(r(width * unit))
                        .h(r(height * unit))
                        .text_color(hsla(palette(cx).avatar.eye))
                        .with_transformation(
                            Transformation::scale(size(1.0, gaze.lid)).with_translation(point(
                                px(gaze.x * unit * zoom),
                                px(gaze.y * unit * zoom),
                            )),
                        ),
                )
            })
    }
}

pub fn bot_color(color: BotColor, cx: &App) -> Rgba {
    let bot = &theme(cx).color.bot;
    match color {
        BotColor::Brown => bot.brown,
        BotColor::Red => bot.red,
        BotColor::Orange => bot.orange,
        BotColor::Amber => bot.amber,
        BotColor::Green => bot.green,
        BotColor::Teal => bot.teal,
        BotColor::Blue => bot.blue,
        BotColor::Purple => bot.purple,
        BotColor::Pink => bot.pink,
        BotColor::Gray => bot.gray,
    }
}

/// The folder name of a shape: `protocol`'s kebab-case name.
fn shape_name(shape: BotShape) -> &'static str {
    match shape {
        BotShape::Circle => "circle",
        BotShape::Blob => "blob",
        BotShape::RoundedSquare => "rounded-square",
        BotShape::Pill => "pill",
        BotShape::Triangle => "triangle",
        BotShape::Hexagon => "hexagon",
        BotShape::Cloud => "cloud",
        BotShape::Droplet => "droplet",
    }
}

/// The eyes' box in the 40-unit avatar: the cropped eyes layer's viewBox.
fn eye_box(shape: BotShape) -> Option<[f32; 4]> {
    let svg = crate::assets::icon(&format!("icons/bot/{}/eyes-crop.svg", shape_name(shape)))?;
    view_box(std::str::from_utf8(&svg).ok()?)
}

fn view_box(svg: &str) -> Option<[f32; 4]> {
    let (_, rest) = svg.split_once("viewBox=\"")?;
    let (numbers, _) = rest.split_once('"')?;
    let mut parsed = numbers.split_whitespace().map(str::parse::<f32>);
    let mut next = || parsed.next()?.ok();
    Some([next()?, next()?, next()?, next()?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shape_has_its_eye_box() {
        let shapes = [
            BotShape::Circle,
            BotShape::Blob,
            BotShape::RoundedSquare,
            BotShape::Pill,
            BotShape::Triangle,
            BotShape::Hexagon,
            BotShape::Cloud,
            BotShape::Droplet,
        ];
        for shape in shapes {
            let [x, y, width, height] =
                eye_box(shape).unwrap_or_else(|| panic!("{shape:?} has no eyes-crop.svg"));
            assert!(
                x >= 0.0
                    && y >= 0.0
                    && width > 0.0
                    && height > 0.0
                    && x + width <= 40.0
                    && y + height <= 40.0,
                "{shape:?}"
            );
        }
        assert_eq!(
            view_box(r#"<svg viewBox="15.5 13.5 11.5 9">"#),
            Some([15.5, 13.5, 11.5, 9.0])
        );
        assert_eq!(view_box("<svg>"), None);
    }
}

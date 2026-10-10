//! Port of Compose Material3 `MaterialShapes` (the 35 Material 3 Expressive shapes).

use std::sync::OnceLock;

use crate::corner_rounding::CornerRounding;
use crate::point::{PI, Point};
use crate::polygon::{RoundedPolygon, StarOptions};

/// The 35 Material 3 Expressive shapes. Use [`MaterialShape::polygon`] for the cached,
/// unit-square-normalized outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MaterialShape {
    Circle,
    Square,
    Slanted,
    Arch,
    Fan,
    Arrow,
    SemiCircle,
    Oval,
    Pill,
    Triangle,
    Diamond,
    ClamShell,
    Pentagon,
    Gem,
    Sunny,
    VerySunny,
    Cookie4Sided,
    Cookie6Sided,
    Cookie7Sided,
    Cookie9Sided,
    Cookie12Sided,
    Ghostish,
    Clover4Leaf,
    Clover8Leaf,
    Burst,
    SoftBurst,
    Boom,
    SoftBoom,
    Flower,
    Puffy,
    PuffyDiamond,
    PixelCircle,
    PixelTriangle,
    Bun,
    Heart,
}

impl MaterialShape {
    /// Every shape, in declaration order.
    pub const ALL: [MaterialShape; 35] = [
        Self::Circle,
        Self::Square,
        Self::Slanted,
        Self::Arch,
        Self::Fan,
        Self::Arrow,
        Self::SemiCircle,
        Self::Oval,
        Self::Pill,
        Self::Triangle,
        Self::Diamond,
        Self::ClamShell,
        Self::Pentagon,
        Self::Gem,
        Self::Sunny,
        Self::VerySunny,
        Self::Cookie4Sided,
        Self::Cookie6Sided,
        Self::Cookie7Sided,
        Self::Cookie9Sided,
        Self::Cookie12Sided,
        Self::Ghostish,
        Self::Clover4Leaf,
        Self::Clover8Leaf,
        Self::Burst,
        Self::SoftBurst,
        Self::Boom,
        Self::SoftBoom,
        Self::Flower,
        Self::Puffy,
        Self::PuffyDiamond,
        Self::PixelCircle,
        Self::PixelTriangle,
        Self::Bun,
        Self::Heart,
    ];

    /// Position in [`MaterialShape::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }

    /// The shape at `index` (wrapping), handy for picking one from a hash or id.
    pub fn from_index(index: usize) -> MaterialShape {
        Self::ALL[index % Self::ALL.len()]
    }

    /// Human-readable name, e.g. `"Cookie9Sided"`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Circle => "Circle",
            Self::Square => "Square",
            Self::Slanted => "Slanted",
            Self::Arch => "Arch",
            Self::Fan => "Fan",
            Self::Arrow => "Arrow",
            Self::SemiCircle => "SemiCircle",
            Self::Oval => "Oval",
            Self::Pill => "Pill",
            Self::Triangle => "Triangle",
            Self::Diamond => "Diamond",
            Self::ClamShell => "ClamShell",
            Self::Pentagon => "Pentagon",
            Self::Gem => "Gem",
            Self::Sunny => "Sunny",
            Self::VerySunny => "VerySunny",
            Self::Cookie4Sided => "Cookie4Sided",
            Self::Cookie6Sided => "Cookie6Sided",
            Self::Cookie7Sided => "Cookie7Sided",
            Self::Cookie9Sided => "Cookie9Sided",
            Self::Cookie12Sided => "Cookie12Sided",
            Self::Ghostish => "Ghostish",
            Self::Clover4Leaf => "Clover4Leaf",
            Self::Clover8Leaf => "Clover8Leaf",
            Self::Burst => "Burst",
            Self::SoftBurst => "SoftBurst",
            Self::Boom => "Boom",
            Self::SoftBoom => "SoftBoom",
            Self::Flower => "Flower",
            Self::Puffy => "Puffy",
            Self::PuffyDiamond => "PuffyDiamond",
            Self::PixelCircle => "PixelCircle",
            Self::PixelTriangle => "PixelTriangle",
            Self::Bun => "Bun",
            Self::Heart => "Heart",
        }
    }

    /// The shape's outline, normalized into `[0, 1]^2` and built once.
    pub fn polygon(self) -> &'static RoundedPolygon {
        static CACHE: [OnceLock<RoundedPolygon>; 35] = [const { OnceLock::new() }; 35];
        CACHE[self.index()].get_or_init(|| self.build().normalized())
    }

    fn build(self) -> RoundedPolygon {
        match self {
            Self::Circle => circle(10),
            Self::Square => square(),
            Self::Slanted => slanted(),
            Self::Arch => arch(),
            Self::Fan => fan(),
            Self::Arrow => arrow(),
            Self::SemiCircle => semi_circle(),
            Self::Oval => oval(),
            Self::Pill => pill(),
            Self::Triangle => triangle(),
            Self::Diamond => diamond(),
            Self::ClamShell => clam_shell(),
            Self::Pentagon => pentagon(),
            Self::Gem => gem(),
            Self::Sunny => sunny(),
            Self::VerySunny => very_sunny(),
            Self::Cookie4Sided => cookie4(),
            Self::Cookie6Sided => cookie6(),
            Self::Cookie7Sided => cookie_star(7, 0.75),
            Self::Cookie9Sided => cookie_star(9, 0.8),
            Self::Cookie12Sided => cookie_star(12, 0.8),
            Self::Ghostish => ghostish(),
            Self::Clover4Leaf => clover4(),
            Self::Clover8Leaf => clover8(),
            Self::Burst => burst(),
            Self::SoftBurst => soft_burst(),
            Self::Boom => boom(),
            Self::SoftBoom => soft_boom(),
            Self::Flower => flower(),
            Self::Puffy => puffy(),
            Self::PuffyDiamond => puffy_diamond(),
            Self::PixelCircle => pixel_circle(),
            Self::PixelTriangle => pixel_triangle(),
            Self::Bun => bun(),
            Self::Heart => heart(),
        }
    }
}

const ROUND15: CornerRounding = CornerRounding::radius(0.15);
const ROUND20: CornerRounding = CornerRounding::radius(0.2);
const ROUND30: CornerRounding = CornerRounding::radius(0.3);
const ROUND50: CornerRounding = CornerRounding::radius(0.5);
const ROUND100: CornerRounding = CornerRounding::radius(1.0);

/// Compose `Matrix.rotateZ(degrees)` applied to a point.
fn rotate_z(degrees: f32) -> impl Fn(f32, f32) -> (f32, f32) {
    let r = f64::from(degrees) * (std::f64::consts::PI / 180.0);
    let (s, c) = (r.sin() as f32, r.cos() as f32);
    move |x, y| (c * x - s * y, s * x + c * y)
}

fn must(p: crate::error::Result<RoundedPolygon>) -> RoundedPolygon {
    match p {
        Ok(p) => p,
        Err(e) => unreachable!("built-in Material shape is valid: {e}"),
    }
}

fn circle(n: usize) -> RoundedPolygon {
    must(RoundedPolygon::circle(n, 1.0, Point::ZERO))
}

fn square() -> RoundedPolygon {
    must(RoundedPolygon::rectangle(
        1.0,
        1.0,
        ROUND30,
        None,
        Point::ZERO,
    ))
}

fn slanted() -> RoundedPolygon {
    custom_polygon(
        &[
            pr(0.926, 0.970, CornerRounding::new(0.189, 0.811)),
            pr(-0.021, 0.967, CornerRounding::new(0.187, 0.057)),
        ],
        2,
        false,
    )
}

fn arch() -> RoundedPolygon {
    must(RoundedPolygon::from_num_vertices(
        4,
        1.0,
        Point::ZERO,
        CornerRounding::UNROUNDED,
        Some(&[ROUND100, ROUND100, ROUND20, ROUND20]),
    ))
    .transformed(rotate_z(-135.0))
}

fn fan() -> RoundedPolygon {
    custom_polygon(
        &[
            pr(1.004, 1.000, CornerRounding::new(0.148, 0.417)),
            pr(0.000, 1.000, CornerRounding::radius(0.151)),
            pr(0.000, -0.003, CornerRounding::radius(0.148)),
            pr(0.978, 0.020, CornerRounding::radius(0.803)),
        ],
        1,
        false,
    )
}

fn arrow() -> RoundedPolygon {
    custom_polygon(
        &[
            pr(0.500, 0.892, CornerRounding::radius(0.313)),
            pr(-0.216, 1.050, CornerRounding::radius(0.207)),
            pr(0.499, -0.160, CornerRounding::new(0.215, 1.000)),
            pr(1.225, 1.060, CornerRounding::radius(0.211)),
        ],
        1,
        false,
    )
}

fn semi_circle() -> RoundedPolygon {
    must(RoundedPolygon::rectangle(
        1.6,
        1.0,
        CornerRounding::UNROUNDED,
        Some(&[ROUND20, ROUND20, ROUND100, ROUND100]),
        Point::ZERO,
    ))
}

fn oval() -> RoundedPolygon {
    must(RoundedPolygon::circle(8, 1.0, Point::ZERO))
        .transformed(|x, y| (x, y * 0.64))
        .transformed(rotate_z(-45.0))
}

fn pill() -> RoundedPolygon {
    custom_polygon(
        &[
            pr(0.961, 0.039, CornerRounding::radius(0.426)),
            pn(1.001, 0.428),
            pr(1.000, 0.609, CornerRounding::radius(1.000)),
        ],
        2,
        true,
    )
}

fn triangle() -> RoundedPolygon {
    must(RoundedPolygon::from_num_vertices(
        3,
        1.0,
        Point::ZERO,
        ROUND20,
        None,
    ))
    .transformed(rotate_z(-90.0))
}

fn diamond() -> RoundedPolygon {
    custom_polygon(
        &[
            pr(0.500, 1.096, CornerRounding::new(0.151, 0.524)),
            pr(0.040, 0.500, CornerRounding::radius(0.159)),
        ],
        2,
        false,
    )
}

fn clam_shell() -> RoundedPolygon {
    custom_polygon(
        &[
            pr(0.171, 0.841, CornerRounding::radius(0.159)),
            pr(-0.020, 0.500, CornerRounding::radius(0.140)),
            pr(0.170, 0.159, CornerRounding::radius(0.159)),
        ],
        2,
        false,
    )
}

fn pentagon() -> RoundedPolygon {
    custom_polygon(
        &[
            pr(0.500, -0.009, CornerRounding::radius(0.172)),
            pr(1.030, 0.365, CornerRounding::radius(0.164)),
            pr(0.828, 0.970, CornerRounding::radius(0.169)),
        ],
        1,
        true,
    )
}

fn gem() -> RoundedPolygon {
    custom_polygon(
        &[
            pr(0.499, 1.023, CornerRounding::new(0.241, 0.778)),
            pr(-0.005, 0.792, CornerRounding::radius(0.208)),
            pr(0.073, 0.258, CornerRounding::radius(0.228)),
            pr(0.433, -0.000, CornerRounding::radius(0.491)),
        ],
        1,
        true,
    )
}

fn sunny() -> RoundedPolygon {
    must(RoundedPolygon::star(&StarOptions {
        num_vertices_per_radius: 8,
        inner_radius: 0.8,
        rounding: ROUND15,
        ..StarOptions::default()
    }))
}

fn very_sunny() -> RoundedPolygon {
    custom_polygon(
        &[
            pr(0.500, 1.080, CornerRounding::radius(0.085)),
            pr(0.358, 0.843, CornerRounding::radius(0.085)),
        ],
        8,
        false,
    )
}

fn cookie4() -> RoundedPolygon {
    custom_polygon(
        &[
            pr(1.237, 1.236, CornerRounding::radius(0.258)),
            pr(0.500, 0.918, CornerRounding::radius(0.233)),
        ],
        4,
        false,
    )
}

fn cookie6() -> RoundedPolygon {
    custom_polygon(
        &[
            pr(0.723, 0.884, CornerRounding::radius(0.394)),
            pr(0.500, 1.099, CornerRounding::radius(0.398)),
        ],
        6,
        false,
    )
}

fn cookie_star(n: usize, inner_radius: f32) -> RoundedPolygon {
    must(RoundedPolygon::star(&StarOptions {
        num_vertices_per_radius: n,
        inner_radius,
        rounding: ROUND50,
        ..StarOptions::default()
    }))
    .transformed(rotate_z(-90.0))
}

fn ghostish() -> RoundedPolygon {
    custom_polygon(
        &[
            pr(0.500, 0.0, CornerRounding::radius(1.000)),
            pr(1.0, 0.0, CornerRounding::radius(1.000)),
            pr(1.0, 1.140, CornerRounding::new(0.254, 0.106)),
            pr(0.575, 0.906, CornerRounding::radius(0.253)),
        ],
        1,
        true,
    )
}

fn clover4() -> RoundedPolygon {
    custom_polygon(
        &[
            pn(0.500, 0.074),
            pr(0.725, -0.099, CornerRounding::radius(0.476)),
        ],
        4,
        true,
    )
}

fn clover8() -> RoundedPolygon {
    custom_polygon(
        &[
            pn(0.500, 0.036),
            pr(0.758, -0.101, CornerRounding::radius(0.209)),
        ],
        8,
        false,
    )
}

fn burst() -> RoundedPolygon {
    custom_polygon(
        &[
            pr(0.500, -0.006, CornerRounding::radius(0.006)),
            pr(0.592, 0.158, CornerRounding::radius(0.006)),
        ],
        12,
        false,
    )
}

fn soft_burst() -> RoundedPolygon {
    custom_polygon(
        &[
            pr(0.193, 0.277, CornerRounding::radius(0.053)),
            pr(0.176, 0.055, CornerRounding::radius(0.053)),
        ],
        10,
        false,
    )
}

fn boom() -> RoundedPolygon {
    custom_polygon(
        &[
            pr(0.457, 0.296, CornerRounding::radius(0.007)),
            pr(0.500, -0.051, CornerRounding::radius(0.007)),
        ],
        15,
        false,
    )
}

fn soft_boom() -> RoundedPolygon {
    custom_polygon(
        &[
            pn(0.733, 0.454),
            pr(0.839, 0.437, CornerRounding::radius(0.532)),
            pr(0.949, 0.449, CornerRounding::new(0.439, 1.000)),
            pr(0.998, 0.478, CornerRounding::radius(0.174)),
        ],
        16,
        true,
    )
}

fn flower() -> RoundedPolygon {
    custom_polygon(
        &[
            pn(0.370, 0.187),
            pr(0.416, 0.049, CornerRounding::radius(0.381)),
            pr(0.479, 0.001, CornerRounding::radius(0.095)),
        ],
        8,
        true,
    )
}

fn puffy() -> RoundedPolygon {
    custom_polygon(
        &[
            pn(0.500, 0.053),
            pr(0.545, -0.040, CornerRounding::radius(0.405)),
            pr(0.670, -0.035, CornerRounding::radius(0.426)),
            pr(0.717, 0.066, CornerRounding::radius(0.574)),
            pn(0.722, 0.128),
            pr(0.777, 0.002, CornerRounding::radius(0.360)),
            pr(0.914, 0.149, CornerRounding::radius(0.660)),
            pr(0.926, 0.289, CornerRounding::radius(0.660)),
            pn(0.881, 0.346),
            pr(0.940, 0.344, CornerRounding::radius(0.126)),
            pr(1.003, 0.437, CornerRounding::radius(0.255)),
        ],
        2,
        true,
    )
    .transformed(|x, y| (x, y * 0.742))
}

fn puffy_diamond() -> RoundedPolygon {
    custom_polygon(
        &[
            pr(0.870, 0.130, CornerRounding::radius(0.146)),
            pn(0.818, 0.357),
            pr(1.000, 0.332, CornerRounding::radius(0.853)),
        ],
        4,
        true,
    )
}

fn pixel_circle() -> RoundedPolygon {
    custom_polygon(
        &[
            pn(0.500, 0.000),
            pn(0.704, 0.000),
            pn(0.704, 0.065),
            pn(0.843, 0.065),
            pn(0.843, 0.148),
            pn(0.926, 0.148),
            pn(0.926, 0.296),
            pn(1.000, 0.296),
        ],
        2,
        true,
    )
}

fn pixel_triangle() -> RoundedPolygon {
    custom_polygon(
        &[
            pn(0.110, 0.500),
            pn(0.113, 0.000),
            pn(0.287, 0.000),
            pn(0.287, 0.087),
            pn(0.421, 0.087),
            pn(0.421, 0.170),
            pn(0.560, 0.170),
            pn(0.560, 0.265),
            pn(0.674, 0.265),
            pn(0.675, 0.344),
            pn(0.789, 0.344),
            pn(0.789, 0.439),
            pn(0.888, 0.439),
        ],
        1,
        true,
    )
}

fn bun() -> RoundedPolygon {
    custom_polygon(
        &[
            pn(0.796, 0.500),
            pr(0.853, 0.518, ROUND100),
            pr(0.992, 0.631, ROUND100),
            pr(0.968, 1.000, ROUND100),
        ],
        2,
        true,
    )
}

fn heart() -> RoundedPolygon {
    custom_polygon(
        &[
            pr(0.500, 0.268, CornerRounding::radius(0.016)),
            pr(0.792, -0.066, CornerRounding::radius(0.958)),
            pr(1.064, 0.276, CornerRounding::radius(1.000)),
            pr(0.501, 0.946, CornerRounding::radius(0.129)),
        ],
        1,
        true,
    )
}

/// A vertex with its corner rounding (`PointNRound`).
#[derive(Debug, Clone, Copy)]
struct PointNRound {
    o: Point,
    r: CornerRounding,
}

fn pr(x: f32, y: f32, r: CornerRounding) -> PointNRound {
    PointNRound {
        o: Point::new(x, y),
        r,
    }
}

fn pn(x: f32, y: f32) -> PointNRound {
    pr(x, y, CornerRounding::UNROUNDED)
}

fn to_radians(deg: f32) -> f32 {
    deg / 360.0 * 2.0 * PI
}

fn rotate_degrees(p: Point, angle: f32, center: Point) -> Point {
    let a = to_radians(angle);
    let off = p - center;
    Point::new(
        off.x * a.cos() - off.y * a.sin(),
        off.x * a.sin() + off.y * a.cos(),
    ) + center
}

fn do_repeat(
    points: &[PointNRound],
    reps: usize,
    center: Point,
    mirroring: bool,
) -> Vec<PointNRound> {
    if mirroring {
        let angles: Vec<f32> = points
            .iter()
            .map(|p| {
                let d = p.o - center;
                d.y.atan2(d.x) * 180.0 / PI
            })
            .collect();
        let distances: Vec<f32> = points.iter().map(|p| (p.o - center).distance()).collect();
        let actual_reps = reps * 2;
        let section_angle = 360.0 / actual_reps as f32;
        let mut out = Vec::with_capacity(actual_reps * points.len());
        for it in 0..actual_reps {
            for index in 0..points.len() {
                let i = if it % 2 == 0 {
                    index
                } else {
                    points.len() - 1 - index
                };
                if i > 0 || it % 2 == 0 {
                    let a = to_radians(
                        section_angle * it as f32
                            + if it % 2 == 0 {
                                angles[i]
                            } else {
                                section_angle - angles[i] + 2.0 * angles[0]
                            },
                    );
                    let p = Point::new(a.cos(), a.sin()) * distances[i] + center;
                    out.push(PointNRound {
                        o: p,
                        r: points[i].r,
                    });
                }
            }
        }
        out
    } else {
        let np = points.len();
        (0..np * reps)
            .map(|it| PointNRound {
                o: rotate_degrees(
                    points[it % np].o,
                    (it / np) as f32 * 360.0 / reps as f32,
                    center,
                ),
                r: points[it % np].r,
            })
            .collect()
    }
}

/// `customPolygon` from MaterialShapes: repeat (and optionally mirror) the points around
/// the center (0.5, 0.5).
fn custom_polygon(pnr: &[PointNRound], reps: usize, mirroring: bool) -> RoundedPolygon {
    let center = Point::new(0.5, 0.5);
    let actual = do_repeat(pnr, reps, center, mirroring);
    let vertices: Vec<f32> = actual.iter().flat_map(|p| [p.o.x, p.o.y]).collect();
    let rounding: Vec<CornerRounding> = actual.iter().map(|p| p.r).collect();
    must(RoundedPolygon::from_vertices(
        &vertices,
        CornerRounding::UNROUNDED,
        Some(&rounding),
        Some(center),
    ))
}

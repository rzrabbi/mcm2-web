//! `.tdf` track definitions: the course a rider follows, and its start and
//! finish lines.
//!
//! Recovered from the loader in the game (0x51b550, `Track.cpp`). The file
//! is a stream of one-byte tags, each followed by its values:
//!
//! ```text
//! 1  i32 node_count         2  f32 x offset          3  f32 z offset
//! 4  f32 scale              5  i32 node (selects)    6  u32 node flags
//! 7  f32 node width         8  new cross-section     9  f32[3] centre
//! 10 f32[3] edge            11 f32[3] other edge     12 u32 (unused)
//! 13 i32 link_count         14 i32 link, i32 node
//! 15 start line f32[10]     16 finish line f32[10]
//! ```
//!
//! Positions are scaled and then offset in x and z; widths and the line
//! extents are scaled. Any other byte is skipped, as the game does, and a
//! few older files rely on that.

use crate::reader::Reader;
use crate::Result;

#[derive(Debug, Clone, Default)]
pub struct TrackDef {
    pub nodes: Vec<Node>,
    pub start: Option<Line>,
    pub finish: Option<Line>,
}

#[derive(Debug, Clone, Default)]
pub struct Node {
    pub flags: u32,
    pub width: f32,
    /// Cross-sections along the course, in driving order.
    pub sections: Vec<CrossSection>,
    /// The nodes this one leads to.
    pub links: Vec<u32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CrossSection {
    pub center: [f32; 3],
    /// The two sides of the course at this point.
    pub edges: [[f32; 3]; 2],
}

/// A start or finish line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    pub position: [f32; 3],
    /// The way riders cross it.
    pub direction: [f32; 3],
    /// From the middle of the line to one end.
    pub across: [f32; 3],
    pub width: f32,
}

impl TrackDef {
    /// Where a race starts: the start line, or else the first stretch of
    /// the course.
    pub fn start_line(&self) -> Option<Line> {
        if self.start.is_some() {
            return self.start;
        }
        let sections = &self.nodes.first()?.sections;
        let (a, b) = (sections.first()?, sections.get(1)?);
        let d = [b.center[0] - a.center[0], 0.0, b.center[2] - a.center[2]];
        let len = (d[0] * d[0] + d[2] * d[2]).sqrt();
        if len == 0.0 {
            return None;
        }
        let across = [
            (a.edges[0][0] - a.edges[1][0]) / 2.0,
            0.0,
            (a.edges[0][2] - a.edges[1][2]) / 2.0,
        ];
        Some(Line {
            position: a.center,
            direction: [d[0] / len, 0.0, d[2] / len],
            across,
            width: 2.0 * (across[0] * across[0] + across[2] * across[2]).sqrt(),
        })
    }
}

pub fn parse(bytes: &[u8]) -> Result<TrackDef> {
    let mut r = Reader::new(bytes, 0);
    let mut def = TrackDef::default();
    let (mut ox, mut oz, mut scale) = (0.0, 0.0, 1.0);
    let mut node: Option<usize> = None;
    let place = |v: [f32; 3], scale: f32, ox: f32, oz: f32| {
        [v[0] * scale + ox, v[1] * scale, v[2] * scale + oz]
    };

    while r.pos < bytes.len() {
        let tag = r.u8("tdf tag")?;
        match tag {
            1 => {
                let count = r.u32("node count")?.min(1 << 16) as usize;
                def.nodes = vec![Node::default(); count];
            }
            2 => ox = r.f32("x offset")?,
            3 => oz = r.f32("z offset")?,
            4 => scale = r.f32("scale")?,
            5 => {
                let i = r.u32("node")? as usize;
                node = (i < def.nodes.len()).then_some(i);
            }
            6 => {
                let flags = r.u32("node flags")?;
                if let Some(n) = node {
                    def.nodes[n].flags = flags | 1;
                }
            }
            7 => {
                let width = r.f32("node width")?;
                if let Some(n) = node {
                    def.nodes[n].width = width * scale;
                }
            }
            8 => {
                if let Some(n) = node {
                    def.nodes[n].sections.push(CrossSection::default());
                }
            }
            9..=11 => {
                let v = vec3(&mut r)?;
                let section = node.and_then(|n| def.nodes[n].sections.last_mut());
                if let Some(s) = section {
                    let v = place(v, scale, ox, oz);
                    match tag {
                        9 => s.center = v,
                        10 => s.edges[0] = v,
                        _ => s.edges[1] = v,
                    }
                }
            }
            12 => {
                r.u32("tdf 12")?;
            }
            13 => {
                let count = r.u32("link count")?.min(1 << 16) as usize;
                if let Some(n) = node {
                    def.nodes[n].links = vec![0; count];
                }
            }
            14 => {
                let i = r.u32("link")? as usize;
                let to = r.u32("link node")?;
                if let Some(link) = node.and_then(|n| def.nodes[n].links.get_mut(i)) {
                    *link = to;
                }
            }
            15 | 16 => {
                let position = place(vec3(&mut r)?, scale, ox, oz);
                let direction = vec3(&mut r)?;
                let across = vec3(&mut r)?.map(|v| v * scale);
                let width = r.f32("line width")? * scale;
                let line = Some(Line {
                    position,
                    direction,
                    across,
                    width,
                });
                if tag == 15 {
                    def.start = line;
                } else {
                    def.finish = line;
                }
            }
            _ => {}
        }
    }
    Ok(def)
}

fn vec3(r: &mut Reader) -> Result<[f32; 3]> {
    Ok([r.f32("vector")?, r.f32("vector")?, r.f32("vector")?])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(out: &mut Vec<u8>, tag: u8, values: &[f32]) {
        out.push(tag);
        for v in values {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }

    #[test]
    fn parses_course_and_lines() {
        let mut b = Vec::new();
        b.extend_from_slice(&[1, 1, 0, 0, 0]);
        f(&mut b, 2, &[100.0]);
        f(&mut b, 3, &[200.0]);
        f(&mut b, 4, &[2.0]);
        b.extend_from_slice(&[5, 0, 0, 0, 0]);
        b.push(0); // skipped, as in some shipped files
        f(&mut b, 7, &[10.0]);
        b.push(8);
        f(&mut b, 9, &[1.0, 0.0, 1.0]);
        f(&mut b, 10, &[2.0, 0.0, 1.0]);
        f(&mut b, 11, &[0.0, 0.0, 1.0]);
        b.push(8);
        f(&mut b, 9, &[1.0, 0.0, 3.0]);
        f(
            &mut b,
            15,
            &[1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 5.0, 0.0, 0.0, 10.0],
        );

        let def = parse(&b).unwrap();
        let node = &def.nodes[0];
        assert_eq!(node.width, 20.0);
        assert_eq!(node.sections.len(), 2);
        assert_eq!(node.sections[0].center, [102.0, 0.0, 202.0]);
        assert_eq!(node.sections[0].edges[1], [100.0, 0.0, 202.0]);
        let start = def.start.unwrap();
        assert_eq!(start.position, [102.0, 0.0, 202.0]);
        assert_eq!(start.width, 20.0);

        // Without a start line, the course's first stretch stands in.
        let course = TrackDef { start: None, ..def };
        let line = course.start_line().unwrap();
        assert_eq!(line.direction, [0.0, 0.0, 1.0]);
        assert_eq!(line.width, 4.0);
    }
}

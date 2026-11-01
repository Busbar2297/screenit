//! Strict argument access for command params: unknown keys, wrong types, non-finite and
//! out-of-range numbers are errors (never panics).

use crate::CommandError;
use screenit_doc::{Color, Style};
use screenit_geom::{Point, PointF, Rect, RectF};
use serde_json::{Map, Value};

pub struct Args<'a> {
    map: Option<&'a Map<String, Value>>,
}

fn perr(m: String) -> CommandError {
    CommandError::Params(m)
}

impl<'a> Args<'a> {
    /// `allowed` lists every accepted key; anything else is an error.
    pub fn new(params: &'a Value, allowed: &[&str]) -> Result<Self, CommandError> {
        let map = match params {
            Value::Null => None,
            Value::Object(m) => Some(m),
            _ => return Err(perr("params must be a JSON object".into())),
        };
        if let Some(m) = map {
            for k in m.keys() {
                if !allowed.contains(&k.as_str()) {
                    return Err(perr(format!(
                        "unknown parameter `{k}` (expected: {})",
                        allowed.join(", ")
                    )));
                }
            }
        }
        Ok(Self { map })
    }

    fn raw(&self, k: &str) -> Option<&'a Value> {
        self.map.and_then(|m| m.get(k)).filter(|v| !v.is_null())
    }

    pub fn has(&self, k: &str) -> bool {
        self.raw(k).is_some()
    }

    pub fn f32(&self, k: &str) -> Result<f32, CommandError> {
        self.opt_f32(k)?
            .ok_or_else(|| perr(format!("missing parameter `{k}`")))
    }

    pub fn opt_f32(&self, k: &str) -> Result<Option<f32>, CommandError> {
        match self.raw(k) {
            None => Ok(None),
            Some(v) => {
                let f = v
                    .as_f64()
                    .ok_or_else(|| perr(format!("`{k}` must be a number")))?;
                if !f.is_finite() || f.abs() > 1.0e9 {
                    return Err(perr(format!("`{k}` is out of range")));
                }
                Ok(Some(f as f32))
            }
        }
    }

    pub fn i32(&self, k: &str) -> Result<i32, CommandError> {
        self.opt_i32(k)?
            .ok_or_else(|| perr(format!("missing parameter `{k}`")))
    }

    pub fn opt_i32(&self, k: &str) -> Result<Option<i32>, CommandError> {
        match self.raw(k) {
            None => Ok(None),
            Some(v) => {
                let n = v
                    .as_i64()
                    .ok_or_else(|| perr(format!("`{k}` must be an integer")))?;
                i32::try_from(n)
                    .map(Some)
                    .map_err(|_| perr(format!("`{k}` is out of range")))
            }
        }
    }

    pub fn u32(&self, k: &str) -> Result<u32, CommandError> {
        self.opt_u32(k)?
            .ok_or_else(|| perr(format!("missing parameter `{k}`")))
    }

    pub fn opt_u32(&self, k: &str) -> Result<Option<u32>, CommandError> {
        match self.raw(k) {
            None => Ok(None),
            Some(v) => {
                let n = v
                    .as_u64()
                    .ok_or_else(|| perr(format!("`{k}` must be a non-negative integer")))?;
                u32::try_from(n)
                    .map(Some)
                    .map_err(|_| perr(format!("`{k}` is out of range")))
            }
        }
    }

    pub fn u64(&self, k: &str) -> Result<u64, CommandError> {
        self.raw(k)
            .ok_or_else(|| perr(format!("missing parameter `{k}`")))?
            .as_u64()
            .ok_or_else(|| perr(format!("`{k}` must be a non-negative integer")))
    }

    pub fn str(&self, k: &str) -> Result<&'a str, CommandError> {
        self.opt_str(k)?
            .ok_or_else(|| perr(format!("missing parameter `{k}`")))
    }

    pub fn opt_str(&self, k: &str) -> Result<Option<&'a str>, CommandError> {
        match self.raw(k) {
            None => Ok(None),
            Some(v) => v
                .as_str()
                .map(Some)
                .ok_or_else(|| perr(format!("`{k}` must be a string"))),
        }
    }

    pub fn bool(&self, k: &str, default: bool) -> Result<bool, CommandError> {
        match self.raw(k) {
            None => Ok(default),
            Some(v) => v
                .as_bool()
                .ok_or_else(|| perr(format!("`{k}` must be a boolean"))),
        }
    }

    pub fn array(&self, k: &str) -> Result<&'a Vec<Value>, CommandError> {
        self.raw(k)
            .ok_or_else(|| perr(format!("missing parameter `{k}`")))?
            .as_array()
            .ok_or_else(|| perr(format!("`{k}` must be an array")))
    }

    /// `[x, y]` or `{"x":..,"y":..}`.
    pub fn pointf(&self, k: &str) -> Result<PointF, CommandError> {
        point_from(
            self.raw(k)
                .ok_or_else(|| perr(format!("missing parameter `{k}`")))?,
            k,
        )
    }

    pub fn opt_pointf(&self, k: &str) -> Result<Option<PointF>, CommandError> {
        self.raw(k).map(|v| point_from(v, k)).transpose()
    }

    /// `[x, y, w, h]` or `{"x","y","w","h"}`.
    pub fn rectf(&self, k: &str) -> Result<RectF, CommandError> {
        rectf_from(
            self.raw(k)
                .ok_or_else(|| perr(format!("missing parameter `{k}`")))?,
            k,
        )
    }

    pub fn color(&self, k: &str) -> Result<Option<Color>, CommandError> {
        match self.opt_str(k)? {
            None => Ok(None),
            Some(s) => Color::from_hex(s)
                .map(Some)
                .ok_or_else(|| perr(format!("`{k}` must be #rrggbb or #rrggbbaa"))),
        }
    }

    /// Optional `style` object: `{stroke, fill, width, shadow}`, merged over `base`.
    pub fn style(&self, base: Style) -> Result<Style, CommandError> {
        let Some(v) = self.raw("style") else {
            return Ok(base);
        };
        let sub = Args::new(v, &["stroke", "fill", "width", "shadow"])?;
        let mut s = base;
        if let Some(c) = sub.color("stroke")? {
            s.stroke = c;
        }
        if sub.has("fill") {
            s.fill = match sub.opt_str("fill")? {
                Some("none") | Some("transparent") => None,
                _ => sub.color("fill")?,
            };
        }
        if let Some(w) = sub.opt_f32("width")? {
            s.width = w;
        }
        s.shadow = sub.bool("shadow", s.shadow)?;
        Ok(s)
    }
}

fn num(v: &Value, k: &str) -> Result<f32, CommandError> {
    let f = v
        .as_f64()
        .ok_or_else(|| perr(format!("`{k}` must contain numbers")))?;
    if !f.is_finite() || f.abs() > 1.0e9 {
        return Err(perr(format!("`{k}` is out of range")));
    }
    Ok(f as f32)
}

fn point_from(v: &Value, k: &str) -> Result<PointF, CommandError> {
    match v {
        Value::Array(a) if a.len() == 2 => Ok(PointF::new(num(&a[0], k)?, num(&a[1], k)?)),
        Value::Object(m) => {
            let g = |n: &str| m.get(n).ok_or_else(|| perr(format!("`{k}` needs `{n}`")));
            Ok(PointF::new(num(g("x")?, k)?, num(g("y")?, k)?))
        }
        _ => Err(perr(format!("`{k}` must be [x, y] or {{x, y}}"))),
    }
}

fn rectf_from(v: &Value, k: &str) -> Result<RectF, CommandError> {
    match v {
        Value::Array(a) if a.len() == 4 => Ok(RectF::new(
            num(&a[0], k)?,
            num(&a[1], k)?,
            num(&a[2], k)?,
            num(&a[3], k)?,
        )),
        Value::Object(m) => {
            let g = |n: &str| m.get(n).ok_or_else(|| perr(format!("`{k}` needs `{n}`")));
            Ok(RectF::new(
                num(g("x")?, k)?,
                num(g("y")?, k)?,
                num(g("w")?, k)?,
                num(g("h")?, k)?,
            ))
        }
        _ => Err(perr(format!(
            "`{k}` must be [x, y, w, h] or {{x, y, w, h}}"
        ))),
    }
}

/// Integer rectangle from `x`,`y`,`w`,`h` keys on the same object.
pub fn rect_keys(a: &Args<'_>) -> Result<Rect, CommandError> {
    Ok(Rect::new(
        a.i32("x")?,
        a.i32("y")?,
        a.u32("w")?,
        a.u32("h")?,
    ))
}

pub fn point_keys(v: &Value) -> Result<Point, CommandError> {
    let p = point_from(v, "point")?;
    Ok(Point::new(p.x as i32, p.y as i32))
}

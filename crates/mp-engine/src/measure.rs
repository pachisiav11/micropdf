//! Measuring: distances, perimeters and areas at a drawing's scale, kept as the Line, PolyLine
//! and Polygon annotations with a Measure dictionary that other readers show as measurements.

use mupdf::pdf::{PdfDocument, PdfObject};

use crate::{DocId, Error};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Measure {
    #[default]
    Distance,
    /// Along an open path of segments.
    Perimeter,
    /// Inside a closed outline.
    Area,
}

/// Units the page side of a scale can be given in, with points per unit.
pub const PAGE_UNITS: [(&str, f32); 4] = [
    ("in", 72.0),
    ("cm", 72.0 / 2.54),
    ("mm", 72.0 / 25.4),
    ("pt", 1.0),
];

/// Units for what the drawing shows.
pub const REAL_UNITS: [&str; 9] = ["in", "ft", "yd", "mi", "mm", "cm", "m", "km", "pt"];

/// How lengths on the page stand for lengths in what the drawing shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Scale {
    /// As readers state it, such as "1 in = 10 ft".
    pub ratio: String,
    /// Units of `unit` per point on the page.
    pub per_point: f32,
    pub unit: String,
}

impl Default for Scale {
    fn default() -> Self {
        Scale {
            ratio: "1 in = 1 in".into(),
            per_point: 1.0 / 72.0,
            unit: "in".into(),
        }
    }
}

/// `v` with at most two decimals, and none that are zero.
fn number(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

impl Scale {
    /// `page` `page_unit`s on the page stand for `real` `unit`s; None for units not listed in
    /// [`PAGE_UNITS`] and [`REAL_UNITS`] or lengths that are not positive.
    pub fn new(page: f32, page_unit: &str, real: f32, unit: &str) -> Option<Scale> {
        let points = PAGE_UNITS.iter().find(|u| u.0 == page_unit)?.1 * page;
        if !(points > 0.0 && real > 0.0 && real.is_finite()) || !REAL_UNITS.contains(&unit) {
            return None;
        }
        Some(Scale {
            ratio: format!("{} {page_unit} = {} {unit}", number(page), number(real)),
            per_point: real / points,
            unit: unit.into(),
        })
    }

    /// The measure of `points`, in page space, as readers write it: "12.5 ft" or "3.2 sq ft".
    pub fn label(&self, kind: Measure, points: &[(f32, f32)]) -> String {
        let length: f32 = points
            .windows(2)
            .map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1))
            .sum();
        match kind {
            Measure::Distance | Measure::Perimeter => {
                format!("{} {}", number(length * self.per_point), self.unit)
            }
            Measure::Area => {
                // The shoelace formula, over the outline closed back to its start.
                let twice: f32 = points
                    .iter()
                    .zip(points.iter().cycle().skip(1))
                    .map(|(a, b)| a.0 * b.1 - b.0 * a.1)
                    .sum();
                let area = twice.abs() / 2.0 * self.per_point * self.per_point;
                format!("{} sq {}", number(area), self.unit)
            }
        }
    }

    /// The Measure dictionary readers convert lengths with: points to `unit` along X and D,
    /// and square units for A.
    pub(crate) fn dictionary(&self, pdf: &PdfDocument) -> Result<PdfObject, Error> {
        let format = |unit: &str, factor: f32| -> Result<PdfObject, Error> {
            let mut f = pdf.new_dict()?;
            f.dict_put("Type", PdfObject::new_name("NumberFormat")?)?;
            f.dict_put("U", PdfObject::new_string(unit)?)?;
            f.dict_put("C", PdfObject::new_real(factor)?)?;
            f.dict_put("D", PdfObject::new_int(100)?)?;
            let mut list = pdf.new_array()?;
            list.array_push(f)?;
            Ok(list)
        };
        let mut m = pdf.new_dict()?;
        m.dict_put("Type", PdfObject::new_name("Measure")?)?;
        m.dict_put("Subtype", PdfObject::new_name("RL")?)?;
        m.dict_put("R", PdfObject::new_string(&self.ratio)?)?;
        m.dict_put("X", format(&self.unit, self.per_point)?)?;
        m.dict_put("D", format(&self.unit, 1.0)?)?;
        m.dict_put("A", format(&format!("sq {}", self.unit), 1.0)?)?;
        Ok(m)
    }
}

/// The scale a page states for its drawing: the first of its viewports with a rectilinear
/// Measure dictionary.
fn page_scale(page: &PdfObject) -> Result<Option<Scale>, Error> {
    let Some(viewports) = page.get_dict("VP")? else {
        return Ok(None);
    };
    if !viewports.is_array()? {
        return Ok(None);
    }
    for view in viewports.array_iter()? {
        let Some(m) = view?.get_dict("Measure")? else {
            continue;
        };
        let rectilinear = m
            .get_dict("Subtype")?
            .map(|s| s.as_name())
            .transpose()?
            .is_none_or(|n| n == b"RL");
        let Some(x) = m
            .get_dict("X")?
            .map(|x| x.get_array(0))
            .transpose()?
            .flatten()
        else {
            continue;
        };
        let per_point = x.get_dict("C")?.map(|c| c.as_float()).transpose()?;
        let unit = x.get_dict("U")?.map(|u| u.as_string()).transpose()?;
        if let (true, Some(per_point), Some(unit)) = (rectilinear, per_point, unit)
            && per_point > 0.0
        {
            let ratio = m.get_dict("R")?.map(|r| r.as_string()).transpose()?;
            return Ok(Some(Scale {
                ratio: ratio.unwrap_or_else(|| format!("1 pt = {} {unit}", number(per_point))),
                per_point,
                unit: unit.trim().to_owned(),
            }));
        }
    }
    Ok(None)
}

impl crate::Engine {
    /// The scale page `page` of `doc` gives its drawing, if it gives one.
    pub fn page_scale(&self, doc: DocId, page: usize) -> Result<Option<Scale>, Error> {
        self.read(doc, move |d, _| {
            let pdf = PdfDocument::try_from(d.clone()).map_err(|_| Error::NotPdf)?;
            page_scale(&pdf.find_page(page as i32)?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_measure_at_the_scale() {
        let scale = Scale::new(1.0, "in", 10.0, "ft").unwrap();
        assert_eq!(scale.ratio, "1 in = 10 ft");
        let square = [(0.0, 0.0), (72.0, 0.0), (72.0, 144.0), (0.0, 144.0)];
        assert_eq!(scale.label(Measure::Distance, &square[..2]), "10 ft");
        assert_eq!(scale.label(Measure::Perimeter, &square), "40 ft");
        assert_eq!(scale.label(Measure::Area, &square), "200 sq ft");
        let metric = Scale::new(1.0, "cm", 2.0, "m").unwrap();
        assert_eq!(
            metric.label(Measure::Distance, &[(0.0, 0.0), (0.0, 72.0)]),
            "5.08 m"
        );
        assert!(Scale::new(0.0, "in", 1.0, "ft").is_none());
        assert!(Scale::new(1.0, "in", 1.0, "parsec").is_none());
    }
}

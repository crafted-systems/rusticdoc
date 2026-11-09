//! Native charts. DOCX, PPTX and XLSX get `c:chartSpace` parts whose values
//! are written inline as caches; ODT, ODS and ODP get an embedded chart
//! document (`Object N/`) that carries its values in a local table, which is
//! how LibreOffice stores a chart that does not read a sheet.

use std::fmt::Write as _;

use minijinja::value::{Kwargs, Value, ValueKind};
use minijinja::Error;

use crate::rich::{esc, fail, points};
use crate::sheet::num_to_col;

/// The most values one chart may hold.
pub const MAX_POINTS: usize = 1000;
/// The most bytes of category labels one chart may hold.
const MAX_LABEL_BYTES: usize = 1024 * 1024;
/// Labels are repeated once per series in the chart part.
const MAX_GENERATED_LABEL_BYTES: usize = 16 * 1024 * 1024;

const DEFAULT_WIDTH_PT: f64 = 450.0;
const DEFAULT_HEIGHT_PT: f64 = 270.0;

/// The colours of the series, or of a pie's slices, in turn: Office's default
/// accents, so that a chart looks alike whichever format it was written in.
/// OOXML charts take them from the theme; ODF charts must spell them out.
const PALETTE: [&str; 6] = ["4472c4", "ed7d31", "a5a5a5", "ffc000", "5b9bd5", "70ad47"];

const ODF_NS: &str = "xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" \
     xmlns:style=\"urn:oasis:names:tc:opendocument:xmlns:style:1.0\" \
     xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\" \
     xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" \
     xmlns:draw=\"urn:oasis:names:tc:opendocument:xmlns:drawing:1.0\" \
     xmlns:fo=\"urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0\" \
     xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
     xmlns:svg=\"urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0\" \
     xmlns:chart=\"urn:oasis:names:tc:opendocument:xmlns:chart:1.0\" office:version=\"1.3\"";

/// The `styles.xml` of an ODF chart object. The chart's look is all in the
/// automatic styles of its content, but LibreOffice expects the part.
pub const ODF_STYLES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
     <office:document-styles xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" \
     office:version=\"1.3\"><office:styles/></office:document-styles>";

const C_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
const A_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bar,
    Column,
    Line,
    Pie,
}

/// The filter's arguments, read before the value is looked at so that
/// validation reports them whatever the sample data holds.
pub struct Options {
    kind: Kind,
    x: String,
    y: Vec<String>,
    title: Option<String>,
    width_pt: f64,
    height_pt: f64,
}

impl Options {
    pub fn read(kwargs: &Kwargs) -> Result<Options, Error> {
        let get = |name: &str| -> Result<Option<Value>, Error> {
            kwargs.get(name).map_err(|e| fail("chart", e))
        };
        let required = |name: &str| -> Result<Value, Error> {
            get(name)?.ok_or_else(|| fail("chart", format!("{name}= is required")))
        };
        let kind = required("type")?;
        let kind = match kind.as_str() {
            Some("bar") => Kind::Bar,
            Some("column") => Kind::Column,
            Some("line") => Kind::Line,
            Some("pie") => Kind::Pie,
            _ => return Err(fail("chart", "type must be bar, column, line or pie")),
        };
        let x = required("x")?;
        let x = x
            .as_str()
            .ok_or_else(|| fail("chart", "x must be a key name"))?
            .to_string();
        let y = required("y")?;
        let y: Vec<String> = if let Some(one) = y.as_str() {
            vec![one.to_string()]
        } else if y.kind() == ValueKind::Seq {
            let mut keys = Vec::new();
            for key in y.try_iter().map_err(|e| fail("chart", e))? {
                let key = key
                    .as_str()
                    .ok_or_else(|| fail("chart", "y must name keys"))?;
                keys.push(key.to_string());
            }
            keys
        } else {
            return Err(fail("chart", "y must be a key name or a list of them"));
        };
        if y.is_empty() {
            return Err(fail("chart", "y must name at least one key"));
        }
        if kind == Kind::Pie && y.len() > 1 {
            return Err(fail("chart", "a pie chart takes one y key"));
        }
        let length = |name: &str, default: f64| -> Result<f64, Error> {
            match get(name)? {
                Some(v) if !v.is_none() => points("chart", name, &v),
                _ => Ok(default),
            }
        };
        Ok(Options {
            kind,
            x,
            y,
            title: get("title")?
                .filter(|t| !t.is_none())
                .map(|t| t.to_string()),
            width_pt: length("width", DEFAULT_WIDTH_PT)?,
            height_pt: length("height", DEFAULT_HEIGHT_PT)?,
        })
    }
}

#[derive(Debug)]
pub struct Series {
    name: String,
    values: Vec<Option<f64>>,
}

/// A checked chart, ready to write.
#[derive(Debug)]
pub struct Chart {
    kind: Kind,
    title: Option<String>,
    categories: Vec<String>,
    series: Vec<Series>,
    pub width_pt: f64,
    pub height_pt: f64,
}

impl Chart {
    /// Builds a chart from a list of objects. A `null` or missing value is a
    /// gap; a series with nothing in it is an error, since that is what a
    /// misspelt key looks like.
    pub fn build(value: &Value, options: Options) -> Result<Chart, String> {
        if value.kind() != ValueKind::Seq {
            return Err(format!("expected a list of objects, got {}", value.kind()));
        }
        let rows = value.len().unwrap_or(0);
        if rows == 0 {
            return Err("the list holds no rows".into());
        }
        let total = rows.saturating_mul(options.y.len());
        if total > MAX_POINTS {
            return Err(format!(
                "the chart has {total} points; at most {MAX_POINTS} are allowed"
            ));
        }
        let mut categories = Vec::with_capacity(rows);
        let mut label_bytes = 0;
        let mut escaped_bytes = 0;
        let mut series: Vec<Series> = options
            .y
            .iter()
            .map(|name| Series {
                name: name.clone(),
                values: Vec::with_capacity(rows),
            })
            .collect();
        for (i, row) in value.try_iter().map_err(|e| e.to_string())?.enumerate() {
            if row.kind() != ValueKind::Map {
                return Err(format!("row {} is not an object", i + 1));
            }
            let category = row.get_attr(&options.x).map_err(|e| e.to_string())?;
            if category.is_undefined() {
                return Err(format!("row {} has no {:?}", i + 1, options.x));
            }
            let label = if category.is_none() {
                String::new()
            } else {
                category.to_string()
            };
            label_bytes += label.len();
            if label_bytes > MAX_LABEL_BYTES {
                return Err(format!(
                    "the category labels are longer than {MAX_LABEL_BYTES} bytes"
                ));
            }
            // Measured escaped, since `&` costs five bytes once written.
            escaped_bytes += esc(&label).len();
            if escaped_bytes.saturating_mul(series.len().max(1)) > MAX_GENERATED_LABEL_BYTES {
                return Err(format!(
                    "the category labels repeated for {} series would pass {MAX_GENERATED_LABEL_BYTES} bytes",
                    series.len()
                ));
            }
            categories.push(label);
            for s in &mut series {
                let v = row.get_attr(&s.name).map_err(|e| e.to_string())?;
                s.values.push(number(&v, &s.name, i)?);
            }
        }
        if let Some(empty) = series.iter().find(|s| s.values.iter().all(Option::is_none)) {
            return Err(format!("no row has a value for {:?}", empty.name));
        }
        Ok(Chart {
            kind: options.kind,
            title: options.title,
            categories,
            series,
            width_pt: options.width_pt,
            height_pt: options.height_pt,
        })
    }

    /// The data as sheet rows: a header, then one row per category, the first
    /// at `first_row`. Text is written as inline strings, never as formulas.
    pub fn sheet_rows(&self, first_row: u32) -> String {
        let mut out = String::new();
        let mut cells = String::new();
        for (k, s) in self.series.iter().enumerate() {
            let at = format!("{}{first_row}", num_to_col(k as u32 + 2));
            let _ = write!(cells, "{}", inline_cell(&at, &s.name));
        }
        let _ = write!(out, "<row r=\"{first_row}\">{cells}</row>");
        for (i, category) in self.categories.iter().enumerate() {
            let r = first_row + 1 + i as u32;
            cells = inline_cell(&format!("A{r}"), category);
            for (k, s) in self.series.iter().enumerate() {
                if let Some(v) = s.values[i] {
                    let col = num_to_col(k as u32 + 2);
                    let _ = write!(cells, "<c r=\"{col}{r}\"><v>{v}</v></c>");
                }
            }
            let _ = write!(out, "<row r=\"{r}\">{cells}</row>");
        }
        out
    }

    /// The bytes of text this chart holds on to.
    pub fn text_bytes(&self) -> usize {
        let labels: usize = self.categories.iter().map(String::len).sum();
        labels + self.series.iter().map(|s| s.name.len()).sum::<usize>()
    }

    /// How many sheet rows [`Chart::sheet_rows`] writes.
    pub fn row_count(&self) -> u32 {
        self.categories.len() as u32 + 1
    }

    /// The chart part. References name `sheet`, starting at `first_row`, where
    /// the data is laid out as in [`Chart::sheet_rows`].
    pub fn xml(&self, sheet: &str, first_row: u32) -> String {
        let last = first_row + self.categories.len() as u32;
        let mut plot = String::new();
        let series: String = self
            .series
            .iter()
            .enumerate()
            .map(|(k, s)| self.series_xml(k, s, sheet, first_row, last))
            .collect();
        match self.kind {
            Kind::Bar | Kind::Column => {
                let dir = if self.kind == Kind::Bar { "bar" } else { "col" };
                let _ = write!(
                    plot,
                    "<c:barChart><c:barDir val=\"{dir}\"/><c:grouping val=\"clustered\"/>\
                     <c:varyColors val=\"0\"/>{series}<c:gapWidth val=\"100\"/>{AX_IDS}\
                     </c:barChart>{}",
                    axes(self.kind == Kind::Bar)
                );
            }
            Kind::Line => {
                let _ = write!(
                    plot,
                    "<c:lineChart><c:grouping val=\"standard\"/><c:varyColors val=\"0\"/>\
                     {series}<c:marker val=\"1\"/>{AX_IDS}</c:lineChart>{}",
                    axes(false)
                );
            }
            Kind::Pie => {
                let _ = write!(
                    plot,
                    "<c:pieChart><c:varyColors val=\"1\"/>{series}\
                     <c:firstSliceAng val=\"0\"/></c:pieChart>"
                );
            }
        }
        let title = match &self.title {
            Some(t) => format!(
                "<c:title><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:rPr lang=\"en-US\"/><a:t>{}</a:t>\
                 </a:r></a:p></c:rich></c:tx><c:overlay val=\"0\"/></c:title>\
                 <c:autoTitleDeleted val=\"0\"/>",
                esc(t)
            ),
            None => "<c:autoTitleDeleted val=\"1\"/>".to_string(),
        };
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
             <c:chartSpace xmlns:c=\"{C_NS}\" xmlns:a=\"{A_NS}\" xmlns:r=\"{R_NS}\">\
             <c:roundedCorners val=\"0\"/><c:chart>{title}<c:plotArea><c:layout/>{plot}\
             </c:plotArea><c:legend><c:legendPos val=\"r\"/><c:overlay val=\"0\"/></c:legend>\
             <c:plotVisOnly val=\"1\"/><c:dispBlanksAs val=\"gap\"/></c:chart></c:chartSpace>"
        )
    }

    /// The `content.xml` of an ODF chart object. The values sit in the
    /// chart's own `local-table`, laid out as in [`Chart::sheet_rows`]: labels
    /// in column A, one column per series, and a header row on top.
    pub fn odf_content(&self) -> String {
        let last_row = self.categories.len() + 1;
        let last_col = num_to_col(self.series.len() as u32 + 1);
        let (class, horizontal) = match self.kind {
            Kind::Bar => ("chart:bar", true),
            Kind::Column => ("chart:bar", false),
            Kind::Line => ("chart:line", false),
            Kind::Pie => ("chart:circle", false),
        };
        let pie = self.kind == Kind::Pie;
        let mut styles = String::new();
        let mut style = |name: &str, body: &str| {
            let _ = write!(
                styles,
                "<style:style style:name=\"{name}\" style:family=\"chart\">{body}</style:style>"
            );
        };
        let text = "<style:text-properties fo:font-size=\"10pt\"/>";
        style(
            "chMain",
            "<style:graphic-properties draw:stroke=\"none\" draw:fill=\"solid\" \
             draw:fill-color=\"#ffffff\"/>",
        );
        style(
            "chTitle",
            "<style:text-properties fo:font-size=\"13pt\" fo:font-weight=\"bold\"/>",
        );
        style("chLegend", text);
        // Markers on lines, and gaps where a value is missing, belong to the plot.
        let symbol = if self.kind == Kind::Line {
            " chart:symbol-type=\"automatic\""
        } else {
            ""
        };
        style(
            "chPlot",
            &format!(
                "<style:chart-properties chart:vertical=\"{horizontal}\" \
                 chart:treat-empty-cells=\"leave-gap\"{symbol}/>"
            ),
        );
        style(
            "chAxis",
            &format!(
                "<style:chart-properties chart:display-label=\"true\"/>\
                 <style:graphic-properties svg:stroke-color=\"#b3b3b3\"/>{text}"
            ),
        );
        style(
            "chGrid",
            "<style:graphic-properties svg:stroke-color=\"#d9d9d9\"/>",
        );
        style(
            "chWall",
            "<style:graphic-properties draw:stroke=\"none\" draw:fill=\"none\"/>",
        );
        style(
            "chPie",
            "<style:graphic-properties draw:stroke=\"solid\" svg:stroke-color=\"#ffffff\"/>",
        );
        let colours = if pie {
            self.categories.len()
        } else {
            self.series.len()
        };
        for k in 0..colours {
            let colour = PALETTE[k % PALETTE.len()];
            let body = if self.kind == Kind::Line {
                format!(
                    "<style:graphic-properties svg:stroke-width=\"0.08cm\" \
                     svg:stroke-color=\"#{colour}\" draw:fill-color=\"#{colour}\"/>"
                )
            } else {
                format!(
                    "<style:graphic-properties draw:stroke=\"none\" draw:fill=\"solid\" \
                     draw:fill-color=\"#{colour}\"/>"
                )
            };
            style(&format!("chS{k}"), &body);
        }

        let title = match &self.title {
            Some(t) => format!(
                "<chart:title chart:style-name=\"chTitle\"><text:p>{}</text:p></chart:title>",
                esc(t)
            ),
            None => String::new(),
        };
        let mut plot = format!(
            "<chart:plot-area chart:style-name=\"chPlot\" \
             table:cell-range-address=\"local-table.$A$1:.${last_col}${last_row}\" \
             chart:data-source-has-labels=\"both\">"
        );
        let categories = format!(
            "<chart:categories table:cell-range-address=\"local-table.$A$2:.$A${last_row}\"/>"
        );
        if pie {
            // A pie draws no axes, but its slices take their names from the
            // categories of the x axis.
            let _ = write!(
                plot,
                "<chart:axis chart:dimension=\"x\" chart:name=\"primary-x\">{categories}\
                 </chart:axis><chart:axis chart:dimension=\"y\" chart:name=\"primary-y\"/>"
            );
        } else {
            let _ = write!(
                plot,
                "<chart:axis chart:dimension=\"x\" chart:name=\"primary-x\" \
                 chart:style-name=\"chAxis\">{categories}</chart:axis>\
                 <chart:axis chart:dimension=\"y\" chart:name=\"primary-y\" \
                 chart:style-name=\"chAxis\"><chart:grid chart:class=\"major\" \
                 chart:style-name=\"chGrid\"/></chart:axis>"
            );
        }
        for k in 0..self.series.len() {
            let col = num_to_col(k as u32 + 2);
            let own_style = if pie {
                " chart:style-name=\"chPie\"".to_string()
            } else {
                format!(" chart:style-name=\"chS{k}\"")
            };
            let _ = write!(
                plot,
                "<chart:series{own_style} chart:class=\"{class}\" \
                 chart:values-cell-range-address=\"local-table.${col}$2:.${col}${last_row}\" \
                 chart:label-cell-address=\"local-table.${col}$1\">"
            );
            if pie {
                for i in 0..self.categories.len() {
                    let _ = write!(plot, "<chart:data-point chart:style-name=\"chS{i}\"/>");
                }
            } else {
                let _ = write!(
                    plot,
                    "<chart:data-point chart:repeated=\"{}\"/>",
                    self.categories.len()
                );
            }
            plot.push_str("</chart:series>");
        }
        if !pie {
            plot.push_str(
                "<chart:wall chart:style-name=\"chWall\"/>\
                 <chart:floor chart:style-name=\"chWall\"/>",
            );
        }
        plot.push_str("</chart:plot-area>");

        let mut table = format!(
            "<table:table table:name=\"local-table\"><table:table-header-columns>\
             <table:table-column/></table:table-header-columns><table:table-columns>\
             <table:table-column table:number-columns-repeated=\"{}\"/></table:table-columns>\
             <table:table-header-rows><table:table-row><table:table-cell><text:p/>\
             </table:table-cell>",
            self.series.len()
        );
        for s in &self.series {
            table.push_str(&odf_text_cell(&s.name));
        }
        table.push_str("</table:table-row></table:table-header-rows><table:table-rows>");
        for (i, category) in self.categories.iter().enumerate() {
            table.push_str("<table:table-row>");
            table.push_str(&odf_text_cell(category));
            for s in &self.series {
                match s.values[i] {
                    Some(v) => {
                        let _ = write!(
                            table,
                            "<table:table-cell office:value-type=\"float\" office:value=\"{v}\">\
                             <text:p>{v}</text:p></table:table-cell>"
                        );
                    }
                    // An empty cell is a gap, as a missing point is in OOXML.
                    None => table.push_str("<table:table-cell/>"),
                }
            }
            table.push_str("</table:table-row>");
        }
        table.push_str("</table:table-rows></table:table>");

        let (w, h) = (self.width_pt, self.height_pt);
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <office:document-content {ODF_NS}><office:automatic-styles>{styles}\
             </office:automatic-styles><office:body><office:chart>\
             <chart:chart svg:width=\"{w:.2}pt\" svg:height=\"{h:.2}pt\" \
             chart:class=\"{class}\" chart:style-name=\"chMain\">{title}\
             <chart:legend chart:legend-position=\"end\" chart:style-name=\"chLegend\"/>\
             {plot}{table}</chart:chart></office:chart></office:body></office:document-content>"
        )
    }

    fn series_xml(&self, k: usize, s: &Series, sheet: &str, first: u32, last: u32) -> String {
        let col = num_to_col(k as u32 + 2);
        let (head, start) = (first, first + 1);
        let mut cats = String::new();
        for (i, c) in self.categories.iter().enumerate() {
            let _ = write!(cats, "<c:pt idx=\"{i}\"><c:v>{}</c:v></c:pt>", esc(c));
        }
        let mut vals = String::new();
        for (i, v) in s.values.iter().enumerate() {
            if let Some(v) = v {
                let _ = write!(vals, "<c:pt idx=\"{i}\"><c:v>{v}</c:v></c:pt>");
            }
        }
        let count = self.categories.len();
        let line_extra = if self.kind == Kind::Line {
            "<c:marker><c:symbol val=\"circle\"/></c:marker>"
        } else {
            ""
        };
        let smooth = if self.kind == Kind::Line {
            "<c:smooth val=\"0\"/>"
        } else {
            ""
        };
        format!(
            "<c:ser><c:idx val=\"{k}\"/><c:order val=\"{k}\"/><c:tx><c:strRef>\
             <c:f>{sheet}!${col}${head}</c:f><c:strCache><c:ptCount val=\"1\"/><c:pt idx=\"0\">\
             <c:v>{name}</c:v></c:pt></c:strCache></c:strRef></c:tx>{line_extra}<c:cat><c:strRef>\
             <c:f>{sheet}!$A${start}:$A${last}</c:f><c:strCache><c:ptCount val=\"{count}\"/>\
             {cats}</c:strCache></c:strRef></c:cat><c:val><c:numRef>\
             <c:f>{sheet}!${col}${start}:${col}${last}</c:f><c:numCache>\
             <c:formatCode>General</c:formatCode><c:ptCount val=\"{count}\"/>{vals}</c:numCache>\
             </c:numRef></c:val>{smooth}</c:ser>",
            name = esc(&s.name)
        )
    }
}

const AX_IDS: &str = "<c:axId val=\"50010\"/><c:axId val=\"50020\"/>";

/// The category and value axes; a horizontal bar chart swaps their sides.
fn axes(horizontal: bool) -> String {
    let (cat_pos, val_pos) = if horizontal { ("l", "b") } else { ("b", "l") };
    format!(
        "<c:catAx><c:axId val=\"50010\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling>\
         <c:delete val=\"0\"/><c:axPos val=\"{cat_pos}\"/><c:numFmt formatCode=\"General\" \
         sourceLinked=\"0\"/><c:majorTickMark val=\"out\"/><c:minorTickMark val=\"none\"/>\
         <c:tickLblPos val=\"nextTo\"/><c:crossAx val=\"50020\"/><c:crosses val=\"autoZero\"/>\
         <c:auto val=\"1\"/><c:lblAlgn val=\"ctr\"/><c:lblOffset val=\"100\"/>\
         <c:noMultiLvlLbl val=\"0\"/></c:catAx>\
         <c:valAx><c:axId val=\"50020\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling>\
         <c:delete val=\"0\"/><c:axPos val=\"{val_pos}\"/><c:majorGridlines/>\
         <c:numFmt formatCode=\"General\" sourceLinked=\"0\"/><c:majorTickMark val=\"out\"/>\
         <c:minorTickMark val=\"none\"/><c:tickLblPos val=\"nextTo\"/><c:crossAx val=\"50010\"/>\
         <c:crosses val=\"autoZero\"/><c:crossBetween val=\"between\"/></c:valAx>"
    )
}

fn odf_text_cell(text: &str) -> String {
    format!(
        "<table:table-cell office:value-type=\"string\"><text:p>{}</text:p></table:table-cell>",
        esc(text)
    )
}

fn inline_cell(at: &str, text: &str) -> String {
    format!(
        "<c r=\"{at}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
        esc(text)
    )
}

fn number(v: &Value, key: &str, row: usize) -> Result<Option<f64>, String> {
    if v.is_undefined() || v.is_none() {
        return Ok(None);
    }
    let n = v
        .is_number()
        .then(|| f64::try_from(v.clone()).ok())
        .flatten()
        .ok_or_else(|| format!("row {}: {key:?} is not a number", row + 1))?;
    if !n.is_finite() {
        return Err(format!("row {}: {key:?} is not a finite number", row + 1));
    }
    Ok(Some(n))
}

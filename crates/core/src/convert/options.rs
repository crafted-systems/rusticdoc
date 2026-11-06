//! Options of a LibreOffice conversion.

use std::fmt;

use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::job::PdfaLevel;

/// Image resolutions LibreOffice's PDF export can reduce to, in DPI.
pub const IMAGE_RESOLUTIONS: [u16; 5] = [75, 150, 300, 600, 1200];

/// Options of a LibreOffice conversion, after Gotenberg's LibreOffice
/// route. Each field maps to a property of LibreOffice's PDF export filter
/// (`writer_pdf_Export`, `calc_pdf_Export`, `impress_pdf_Export`, ...);
/// they are ignored for non-PDF targets. `landscape` is applied to the
/// loaded document's page styles before export, best effort. `password`
/// opens a protected input.
#[derive(Clone, PartialEq, Eq)]
pub struct ConvertOptions {
    /// PDF/A level, `SelectPdfVersion`.
    pub pdfa: Option<PdfaLevel>,
    /// `PDFUACompliance`.
    pub pdfua: bool,
    /// Turn every page to landscape before exporting.
    pub landscape: bool,
    /// LibreOffice's `PageRange`, for example `1-3,5`.
    pub native_page_ranges: Option<String>,
    /// `ExportFormFields`, on by default.
    pub export_form_fields: bool,
    /// `AllowDuplicateFieldNames`.
    pub allow_duplicate_field_names: bool,
    /// `ExportBookmarks`, on by default.
    pub export_bookmarks: bool,
    /// `ExportBookmarksToPDFDestination`.
    pub export_bookmarks_to_pdf_destination: bool,
    /// `ExportPlaceholders`.
    pub export_placeholders: bool,
    /// `ExportNotes`.
    pub export_notes: bool,
    /// `ExportNotesPages` (Impress).
    pub export_notes_pages: bool,
    /// `ExportOnlyNotesPages` (Impress).
    pub export_only_notes_pages: bool,
    /// `ExportNotesInMargin` (Writer).
    pub export_notes_in_margin: bool,
    /// `ConvertOOoTargetToPDFTarget`.
    pub convert_ooo_target_to_pdf_target: bool,
    /// `ExportLinksRelativeFsys`.
    pub export_links_relative_fsys: bool,
    /// `ExportHiddenSlides` (Impress).
    pub export_hidden_slides: bool,
    /// `IsSkipEmptyPages`.
    pub skip_empty_pages: bool,
    /// `IsAddStream`: embed the source document in the PDF.
    pub add_original_document_as_stream: bool,
    /// `SinglePageSheets` (Calc): one page per sheet.
    pub single_page_sheets: bool,
    /// `UseLosslessCompression` for images.
    pub lossless_image_compression: bool,
    /// JPEG `Quality`, 1 to 100, default 90.
    pub quality: u8,
    /// `ReduceImageResolution`.
    pub reduce_image_resolution: bool,
    /// `MaxImageResolution` in DPI, one of [`IMAGE_RESOLUTIONS`], default 300.
    pub max_image_resolution: u16,
    /// Password that opens a protected input document.
    pub password: Option<String>,
    /// Fonts for this conversion alone, as (name, bytes). The job runs on
    /// a dedicated worker that is discarded afterwards, so it is slower.
    pub fonts: Vec<(String, Vec<u8>)>,
    /// `UseTaggedPDF`, implied by `pdfua`.
    pub generate_tagged_pdf: bool,
    /// Refresh indexes and tables of contents before exporting.
    pub update_indexes: bool,
    /// `InitialView`: 0 page only, 1 bookmarks, 2 thumbnails.
    pub initial_view: i32,
    /// `InitialPage`, from 1.
    pub initial_page: i32,
    /// `Magnification`: 0 default, 1 fit page, 2 fit width, 3 fit visible, 4 zoom.
    pub magnification: i32,
    /// `Zoom`, in percent, with magnification 4.
    pub zoom: i32,
    /// `PageLayout`: 0 default, 1 single, 2 one column, 3 two columns.
    pub page_layout: i32,
    /// `FirstPageOnLeft`.
    pub first_page_on_left: bool,
    /// `ResizeWindowToInitialPage`.
    pub resize_window_to_initial_page: bool,
    /// `CenterWindow`.
    pub center_window: bool,
    /// `OpenInFullScreenMode`.
    pub open_in_full_screen_mode: bool,
    /// `DisplayPDFDocumentTitle`, on by default.
    pub display_pdf_document_title: bool,
    /// `HideViewerMenubar`.
    pub hide_viewer_menubar: bool,
    /// `HideViewerToolbar`.
    pub hide_viewer_toolbar: bool,
    /// `HideViewerWindowControls`.
    pub hide_viewer_window_controls: bool,
    /// `UseTransitionEffects`, on by default.
    pub use_transition_effects: bool,
    /// `OpenBookmarkLevels`: -1 for all, or 1 to 10.
    pub open_bookmark_levels: i32,
    /// `Watermark`: LibreOffice's own text watermark.
    pub native_watermark_text: Option<String>,
    /// `WatermarkColor`, an RGB integer, with `Watermark`.
    pub native_watermark_color: Option<i32>,
    /// `WatermarkFontHeight`, with `Watermark`.
    pub native_watermark_font_height: Option<i32>,
    /// `WatermarkRotateAngle`, in tenths of a degree, with `Watermark`.
    pub native_watermark_rotate_angle: Option<i32>,
    /// `WatermarkFontName`, with `Watermark`.
    pub native_watermark_font_name: Option<String>,
    /// `TiledWatermark`.
    pub native_tiled_watermark_text: Option<String>,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            pdfa: None,
            pdfua: false,
            landscape: false,
            native_page_ranges: None,
            export_form_fields: true,
            allow_duplicate_field_names: false,
            export_bookmarks: true,
            export_bookmarks_to_pdf_destination: false,
            export_placeholders: false,
            export_notes: false,
            export_notes_pages: false,
            export_only_notes_pages: false,
            export_notes_in_margin: false,
            convert_ooo_target_to_pdf_target: false,
            export_links_relative_fsys: false,
            export_hidden_slides: false,
            skip_empty_pages: false,
            add_original_document_as_stream: false,
            single_page_sheets: false,
            lossless_image_compression: false,
            quality: 90,
            reduce_image_resolution: false,
            max_image_resolution: 300,
            password: None,
            fonts: Vec::new(),
            generate_tagged_pdf: false,
            update_indexes: false,
            initial_view: 0,
            initial_page: 1,
            magnification: 0,
            zoom: 100,
            page_layout: 0,
            first_page_on_left: false,
            resize_window_to_initial_page: false,
            center_window: false,
            open_in_full_screen_mode: false,
            display_pdf_document_title: true,
            hide_viewer_menubar: false,
            hide_viewer_toolbar: false,
            hide_viewer_window_controls: false,
            use_transition_effects: true,
            open_bookmark_levels: -1,
            native_watermark_text: None,
            native_watermark_color: None,
            native_watermark_font_height: None,
            native_watermark_rotate_angle: None,
            native_watermark_font_name: None,
            native_tiled_watermark_text: None,
        }
    }
}

impl ConvertOptions {
    /// Options that only set the PDF/A level, as the plain `convert` call
    /// does.
    pub fn pdfa(level: Option<PdfaLevel>) -> Self {
        Self {
            pdfa: level,
            ..Self::default()
        }
    }

    /// Rejects values LibreOffice would refuse or silently change.
    pub fn validate(&self) -> Result<()> {
        crate::fonts::check_uploads(&self.fonts)?;
        if !(1..=100).contains(&self.quality) {
            return Err(Error::InvalidRequest(format!(
                "quality must be between 1 and 100, not {}",
                self.quality
            )));
        }
        if !IMAGE_RESOLUTIONS.contains(&self.max_image_resolution) {
            return Err(Error::InvalidRequest(format!(
                "maxImageResolution must be one of {:?}, not {}",
                IMAGE_RESOLUTIONS, self.max_image_resolution
            )));
        }
        let is_range_char = |c: char| c.is_ascii_digit() || c == '-' || c == ',' || c == ' ';
        if let Some(ranges) = self
            .native_page_ranges
            .as_deref()
            .filter(|r| !r.chars().all(is_range_char))
        {
            return Err(Error::InvalidRequest(format!(
                "nativePageRanges '{ranges}' is not a page range like '1-3,5'"
            )));
        }
        Ok(())
    }

    /// The `FilterData` of the PDF export: booleans, longs and strings keyed
    /// by LibreOffice's property names.
    pub fn filter_data(&self) -> Map<String, Value> {
        let mut m = Map::new();
        let mut flag = |name: &str, value: bool| {
            m.insert(name.to_string(), Value::Bool(value));
        };
        flag("PDFUACompliance", self.pdfua);
        flag("UseTaggedPDF", self.pdfua || self.generate_tagged_pdf);
        flag("EnableTextAccessForAccessibilityTools", self.pdfua);
        flag("EmbedStandardFonts", self.pdfua || self.pdfa.is_some());
        flag("FirstPageOnLeft", self.first_page_on_left);
        flag("ResizeWindowToInitialPage", self.resize_window_to_initial_page);
        flag("CenterWindow", self.center_window);
        flag("OpenInFullScreenMode", self.open_in_full_screen_mode);
        flag("DisplayPDFDocumentTitle", self.display_pdf_document_title);
        flag("HideViewerMenubar", self.hide_viewer_menubar);
        flag("HideViewerToolbar", self.hide_viewer_toolbar);
        flag("HideViewerWindowControls", self.hide_viewer_window_controls);
        flag("UseTransitionEffects", self.use_transition_effects);
        flag("ExportFormFields", self.export_form_fields);
        flag("AllowDuplicateFieldNames", self.allow_duplicate_field_names);
        flag("ExportBookmarks", self.export_bookmarks);
        flag(
            "ExportBookmarksToPDFDestination",
            self.export_bookmarks_to_pdf_destination,
        );
        flag("ExportPlaceholders", self.export_placeholders);
        flag("ExportNotes", self.export_notes);
        flag("ExportNotesPages", self.export_notes_pages);
        flag("ExportOnlyNotesPages", self.export_only_notes_pages);
        flag("ExportNotesInMargin", self.export_notes_in_margin);
        flag(
            "ConvertOOoTargetToPDFTarget",
            self.convert_ooo_target_to_pdf_target,
        );
        flag("ExportLinksRelativeFsys", self.export_links_relative_fsys);
        flag("ExportHiddenSlides", self.export_hidden_slides);
        flag("IsSkipEmptyPages", self.skip_empty_pages);
        flag("IsAddStream", self.add_original_document_as_stream);
        flag("SinglePageSheets", self.single_page_sheets);
        flag("UseLosslessCompression", self.lossless_image_compression);
        flag("ReduceImageResolution", self.reduce_image_resolution);
        m.insert("Quality".into(), Value::from(self.quality));
        m.insert(
            "MaxImageResolution".into(),
            Value::from(self.max_image_resolution),
        );
        m.insert("InitialView".into(), Value::from(self.initial_view));
        m.insert("InitialPage".into(), Value::from(self.initial_page));
        m.insert("Magnification".into(), Value::from(self.magnification));
        m.insert("Zoom".into(), Value::from(self.zoom));
        m.insert("PageLayout".into(), Value::from(self.page_layout));
        m.insert(
            "OpenBookmarkLevels".into(),
            Value::from(self.open_bookmark_levels),
        );
        if let Some(text) = &self.native_watermark_text {
            m.insert("Watermark".into(), Value::from(text.as_str()));
        }
        if let Some(color) = self.native_watermark_color {
            m.insert("WatermarkColor".into(), Value::from(color));
        }
        if let Some(height) = self.native_watermark_font_height {
            m.insert("WatermarkFontHeight".into(), Value::from(height));
        }
        if let Some(angle) = self.native_watermark_rotate_angle {
            m.insert("WatermarkRotateAngle".into(), Value::from(angle));
        }
        if let Some(name) = &self.native_watermark_font_name {
            m.insert("WatermarkFontName".into(), Value::from(name.as_str()));
        }
        if let Some(text) = &self.native_tiled_watermark_text {
            m.insert("TiledWatermark".into(), Value::from(text.as_str()));
        }
        if let Some(level) = self.pdfa {
            m.insert(
                "SelectPdfVersion".into(),
                Value::from(level.libreoffice_version()),
            );
        }
        if let Some(ranges) = &self.native_page_ranges {
            m.insert("PageRange".into(), Value::from(ranges.trim()));
        }
        m
    }
}

impl fmt::Debug for ConvertOptions {
    /// The password is never printed.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConvertOptions")
            .field("filter_data", &self.filter_data())
            .field("landscape", &self.landscape)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("fonts", &self.fonts.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_gotenberg() {
        let data = ConvertOptions::default().filter_data();
        assert_eq!(data["ExportFormFields"], true);
        assert_eq!(data["ExportBookmarks"], true);
        assert_eq!(data["Quality"], 90);
        assert_eq!(data["MaxImageResolution"], 300);
        assert!(!data.contains_key("SelectPdfVersion"));
        assert!(!data.contains_key("PageRange"));
    }

    #[test]
    fn pdfa_and_ranges_are_added() {
        let mut o = ConvertOptions::pdfa(Some(PdfaLevel::A3b));
        o.native_page_ranges = Some(" 1-2 ".into());
        let data = o.filter_data();
        assert_eq!(data["SelectPdfVersion"], 3);
        assert_eq!(data["PageRange"], "1-2");
    }

    #[test]
    fn bad_values_are_client_errors() {
        let mut o = ConvertOptions {
            quality: 0,
            ..Default::default()
        };
        assert!(o.validate().unwrap_err().is_client_error());
        o.quality = 50;
        o.max_image_resolution = 299;
        assert!(o.validate().is_err());
        o.max_image_resolution = 600;
        o.native_page_ranges = Some("1;rm".into());
        assert!(o.validate().is_err());
        o.native_page_ranges = Some("1-3, 5".into());
        o.validate().unwrap();
    }

    #[test]
    fn debug_hides_the_password() {
        let o = ConvertOptions {
            password: Some("hunter2".into()),
            ..Default::default()
        };
        let s = format!("{o:?}");
        assert!(s.contains("redacted") && !s.contains("hunter2"));
    }
}

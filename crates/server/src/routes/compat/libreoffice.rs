//! `/forms/libreoffice/convert`.

use axum::extract::{Request, State};
use axum::response::Response;
use rusticdoc_core::convert::ConvertOptions;

use super::form::{self, FormData};
use super::{Reply, client_error, engines, run};
use crate::error::ApiResult;
use crate::state::AppState;

/// What LibreOffice is sent. Anything else in a form is ignored.
const EXTENSIONS: &[&str] = &[
    ".123", ".602", ".abw", ".bib", ".bmp", ".cdr", ".cgm", ".cmx", ".csv", ".cwk", ".dbf",
    ".dif", ".doc", ".docm", ".docx", ".dot", ".dotm", ".dotx", ".dxf", ".emf", ".eps", ".epub",
    ".fodg", ".fodp", ".fods", ".fodt", ".fopd", ".gif", ".htm", ".html", ".hwp", ".jpeg",
    ".jpg", ".key", ".ltx", ".lwp", ".mcw", ".met", ".mml", ".mw", ".numbers", ".odd", ".odg",
    ".odm", ".odp", ".ods", ".odt", ".otg", ".oth", ".otp", ".ots", ".ott", ".pages", ".pbm",
    ".pcd", ".pct", ".pcx", ".pdb", ".pdf", ".pgm", ".png", ".pot", ".potm", ".potx", ".ppm",
    ".pps", ".ppsm", ".ppsx", ".ppt", ".pptm", ".pptx", ".psd", ".psw", ".pub", ".pwp", ".pxl",
    ".ras", ".rtf", ".sda", ".sdc", ".sdd", ".sdp", ".sdw", ".sgl", ".slk", ".smf", ".stc",
    ".std", ".sti", ".stw", ".svg", ".svm", ".swf", ".sxc", ".sxd", ".sxg", ".sxi", ".sxm",
    ".sxw", ".tga", ".tif", ".tiff", ".txt", ".uof", ".uop", ".uos", ".uot", ".vdx", ".vor",
    ".vsd", ".vsdm", ".vsdx", ".wb2", ".wk1", ".wks", ".wmf", ".wpd", ".wpg", ".wps", ".xbm",
    ".xhtml", ".xls", ".xlsb", ".xlsm", ".xlsx", ".xlt", ".xltm", ".xltx", ".xlw", ".xml",
    ".xpm", ".zabw",
];

fn int_in(
    form: &mut FormData,
    key: &str,
    default: i32,
    check: impl FnOnce(i64) -> Result<(), String>,
) -> i32 {
    form.custom(key, |value| {
        if value.is_empty() {
            return Ok(default);
        }
        let n = value.parse::<i64>().map_err(|_| {
            format!("strconv.Atoi: parsing {value:?}: invalid syntax")
        })?;
        check(n)?;
        i32::try_from(n).map_err(|_| format!("strconv.Atoi: parsing {value:?}: value out of range"))
    })
    .unwrap_or(default)
}

/// The export options, read in the order their errors are listed.
fn read_options(form: &mut FormData) -> ConvertOptions {
    let mut o = ConvertOptions {
        update_indexes: true,
        ..ConvertOptions::default()
    };
    o.password = form.optional("password");
    o.landscape = form.boolean("landscape", o.landscape);
    o.native_page_ranges = form.optional("nativePageRanges");
    o.update_indexes = form.boolean("updateIndexes", o.update_indexes);
    o.export_form_fields = form.boolean("exportFormFields", o.export_form_fields);
    o.allow_duplicate_field_names =
        form.boolean("allowDuplicateFieldNames", o.allow_duplicate_field_names);
    o.export_bookmarks = form.boolean("exportBookmarks", o.export_bookmarks);
    o.export_bookmarks_to_pdf_destination = form.boolean(
        "exportBookmarksToPdfDestination",
        o.export_bookmarks_to_pdf_destination,
    );
    o.export_placeholders = form.boolean("exportPlaceholders", o.export_placeholders);
    o.export_notes = form.boolean("exportNotes", o.export_notes);
    o.export_notes_pages = form.boolean("exportNotesPages", o.export_notes_pages);
    o.export_only_notes_pages = form.boolean("exportOnlyNotesPages", o.export_only_notes_pages);
    o.export_notes_in_margin = form.boolean("exportNotesInMargin", o.export_notes_in_margin);
    o.convert_ooo_target_to_pdf_target = form.boolean(
        "convertOooTargetToPdfTarget",
        o.convert_ooo_target_to_pdf_target,
    );
    o.export_links_relative_fsys =
        form.boolean("exportLinksRelativeFsys", o.export_links_relative_fsys);
    o.export_hidden_slides = form.boolean("exportHiddenSlides", o.export_hidden_slides);
    o.skip_empty_pages = form.boolean("skipEmptyPages", o.skip_empty_pages);
    o.add_original_document_as_stream = form.boolean(
        "addOriginalDocumentAsStream",
        o.add_original_document_as_stream,
    );
    o.single_page_sheets = form.boolean("singlePageSheets", o.single_page_sheets);
    o.generate_tagged_pdf = form.boolean("generateTaggedPdf", o.generate_tagged_pdf);
    o.initial_view = int_in(form, "initialView", o.initial_view, |n| {
        if (0..=2).contains(&n) {
            Ok(())
        } else {
            Err("value is not 0, 1 or 2".into())
        }
    });
    o.initial_page = int_in(form, "initialPage", o.initial_page, |n| {
        if n >= 1 {
            Ok(())
        } else {
            Err("value is inferior to 1".into())
        }
    });
    o.magnification = int_in(form, "magnification", o.magnification, |n| {
        if (0..=4).contains(&n) {
            Ok(())
        } else {
            Err("value is not 0, 1, 2, 3 or 4".into())
        }
    });
    o.zoom = int_in(form, "zoom", o.zoom, |n| {
        if n >= 1 {
            Ok(())
        } else {
            Err("value is inferior to 1".into())
        }
    });
    o.page_layout = int_in(form, "pageLayout", o.page_layout, |n| {
        if (0..=3).contains(&n) {
            Ok(())
        } else {
            Err("value is not 0, 1, 2 or 3".into())
        }
    });
    o.first_page_on_left = form.boolean("firstPageOnLeft", o.first_page_on_left);
    o.resize_window_to_initial_page =
        form.boolean("resizeWindowToInitialPage", o.resize_window_to_initial_page);
    o.center_window = form.boolean("centerWindow", o.center_window);
    o.open_in_full_screen_mode = form.boolean("openInFullScreenMode", o.open_in_full_screen_mode);
    o.display_pdf_document_title =
        form.boolean("displayPDFDocumentTitle", o.display_pdf_document_title);
    o.hide_viewer_menubar = form.boolean("hideViewerMenubar", o.hide_viewer_menubar);
    o.hide_viewer_toolbar = form.boolean("hideViewerToolbar", o.hide_viewer_toolbar);
    o.hide_viewer_window_controls =
        form.boolean("hideViewerWindowControls", o.hide_viewer_window_controls);
    o.use_transition_effects = form.boolean("useTransitionEffects", o.use_transition_effects);
    o.open_bookmark_levels = int_in(form, "openBookmarkLevels", o.open_bookmark_levels, |n| {
        if n == -1 || (1..=10).contains(&n) {
            Ok(())
        } else {
            Err("value is not -1 or between 1 and 10".into())
        }
    });
    o.lossless_image_compression =
        form.boolean("losslessImageCompression", o.lossless_image_compression);
    o.quality = int_in(form, "quality", i32::from(o.quality), |n| {
        if n < 1 {
            Err("value is inferior to 1".into())
        } else if n > 100 {
            Err("value is superior to 100".into())
        } else {
            Ok(())
        }
    }) as u8;
    o.reduce_image_resolution = form.boolean("reduceImageResolution", o.reduce_image_resolution);
    o.max_image_resolution = int_in(
        form,
        "maxImageResolution",
        i32::from(o.max_image_resolution),
        |n| {
            if matches!(n, 75 | 150 | 300 | 600 | 1200) {
                Ok(())
            } else {
                Err("value is not 75, 150, 300, 600 or 1200".into())
            }
        },
    ) as u16;
    o.native_watermark_text = form.optional("nativeWatermarkText");
    let color = int_in(form, "nativeWatermarkColor", 8388223, |_| Ok(()));
    let height = int_in(form, "nativeWatermarkFontHeight", 0, |n| {
        if n >= 0 {
            Ok(())
        } else {
            Err("value is inferior to 0".into())
        }
    });
    let angle = int_in(form, "nativeWatermarkRotateAngle", 0, |_| Ok(()));
    if o.native_watermark_text.is_some() {
        o.native_watermark_color = Some(color);
        o.native_watermark_font_height = (height > 0).then_some(height);
        o.native_watermark_rotate_angle = (angle != 0).then_some(angle);
    }
    o.native_watermark_font_name = form
        .optional("nativeWatermarkFontName")
        .filter(|name| name != "Helvetica");
    o.native_tiled_watermark_text = form.optional("nativeTiledWatermarkText");
    o.fonts = form.fonts();
    o
}

pub async fn convert(State(state): State<AppState>, req: Request) -> Response {
    run(state, req, handle).await
}

async fn handle(state: AppState, mut form: FormData) -> ApiResult<Reply> {
    let split = form::read_split(&mut form, false);
    let steps = form::read_steps(&mut form, false)?;
    let inputs = form.mandatory_paths(EXTENSIONS);
    let options = read_options(&mut form);
    let native_formats = form.boolean("nativePdfFormats", true);
    let merge = form.boolean("merge", false);
    let flatten = form.boolean("flatten", false);
    form.validate()?;
    let steps = steps.bind(flatten)?;
    let docs = engines(&state)
        .libreoffice(
            inputs,
            &options,
            native_formats,
            merge,
            split.as_ref(),
            &steps,
        )
        .await
        .map_err(client_error)?;
    Ok(Reply::Files(docs))
}

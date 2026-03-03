use serde::Serialize;
use std::fs::{self, File};
use std::io::{BufReader, Read, Seek, SeekFrom};
use exif::{Reader, Tag, In};
use base64::{Engine as _, engine::general_purpose};

#[derive(Serialize, Clone)]
struct RawInfo {
    width: usize,
    height: usize,
    make: String,
    model: String,
    clean_make: String,
    clean_model: String,
    iso: Option<String>,
    shutter_speed: Option<String>,
    aperture: Option<String>,
    focal_length: Option<String>,
    lens_model: Option<String>,
    date_taken: Option<String>,
    file_size_bytes: u64,
    thumbnail_base64: Option<String>,
}

#[tauri::command]
fn list_arw_files(dir_path: &str) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    let entries = fs::read_dir(dir_path).map_err(|e| e.to_string())?;
    
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension() {
                if ext.to_string_lossy().to_lowercase() == "arw" {
                    files.push(path.to_string_lossy().into_owned());
                }
            }
        }
    }
    
    Ok(files)
}

// Run on async thread pool so it NEVER blocks the main/UI thread.
// This is the key fix: Tauri's default sync commands run on the main
// thread and block the entire WebView if they do heavy I/O.
#[tauri::command(async)]
fn load_raw_info(path: String) -> Result<RawInfo, String> {
    // Only read the first 256KB of the file for EXIF parsing.
    // ARW EXIF/TIFF headers + IFD0 thumbnail are always within the
    // first ~64KB. 256KB gives generous headroom while avoiding
    // reading the full 60MB RAW payload.
    let file = File::open(&path).map_err(|e| e.to_string())?;
    let file_size_bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
    let limit = std::cmp::min(file_size_bytes, 256 * 1024) as usize;
    
    let mut header_buf = vec![0u8; limit];
    let mut bufreader = BufReader::new(&file);
    bufreader.read_exact(&mut header_buf).map_err(|e| e.to_string())?;
    
    // Parse EXIF from the header buffer only
    let mut cursor = std::io::Cursor::new(&header_buf);
    let exif = Reader::new().read_from_container(&mut cursor).map_err(|e| e.to_string())?;

    let make = match exif.get_field(Tag::Make, In::PRIMARY) {
        Some(f) => f.display_value().with_unit(&exif).to_string().replace("\"", ""),
        None => "Unknown".to_string(),
    };

    let model = match exif.get_field(Tag::Model, In::PRIMARY) {
        Some(f) => f.display_value().with_unit(&exif).to_string().replace("\"", ""),
        None => "Unknown".to_string(),
    };

    let width = match exif.get_field(Tag::PixelXDimension, In::PRIMARY) {
        Some(f) => f.value.get_uint(0).unwrap_or(0) as usize,
        None => 0,
    };

    let height = match exif.get_field(Tag::PixelYDimension, In::PRIMARY) {
        Some(f) => f.value.get_uint(0).unwrap_or(0) as usize,
        None => 0,
    };

    // Helper to get string from EXIF field
    let get_str = |tag: Tag| -> Option<String> {
        exif.get_field(tag, In::PRIMARY)
            .map(|f| f.display_value().with_unit(&exif).to_string().replace("\"", ""))
    };

    let iso = get_str(Tag::PhotographicSensitivity)
        .or_else(|| get_str(Tag::ISOSpeed));
    let shutter_speed = get_str(Tag::ExposureTime);
    let aperture = get_str(Tag::FNumber);
    let focal_length = get_str(Tag::FocalLength);
    let lens_model = get_str(Tag::LensModel);
    let date_taken = get_str(Tag::DateTimeOriginal);

    // Extract thumbnail from EXIF IFD1 (small JPEG, typically ~10-30KB)
    let mut thumbnail_base64 = None;

    if let (Some(offset_field), Some(length_field)) = (
        exif.get_field(Tag::JPEGInterchangeFormat, In::PRIMARY),
        exif.get_field(Tag::JPEGInterchangeFormatLength, In::PRIMARY),
    ) {
        if let (Some(offset), Some(length)) = (
            offset_field.value.get_uint(0),
            length_field.value.get_uint(0),
        ) {
            let offset = offset as u64;
            let length = length as usize;
            
            // Cap thumbnail read at 512KB to avoid accidentally reading
            // a massive full-res preview
            if length <= 512 * 1024 {
                // If thumbnail is within our header buffer, use it directly
                if (offset as usize + length) <= header_buf.len() {
                    let thumb = &header_buf[offset as usize..offset as usize + length];
                    let b64 = general_purpose::STANDARD.encode(thumb);
                    thumbnail_base64 = Some(format!("data:image/jpeg;base64,{}", b64));
                } else {
                    // Otherwise, do a targeted seek+read from the original file
                    let mut buf = vec![0u8; length];
                    if bufreader.seek(SeekFrom::Start(offset)).is_ok() {
                        if bufreader.read_exact(&mut buf).is_ok() {
                            let b64 = general_purpose::STANDARD.encode(&buf);
                            thumbnail_base64 = Some(format!("data:image/jpeg;base64,{}", b64));
                        }
                    }
                }
            }
        }
    }

    Ok(RawInfo {
        width,
        height,
        make: make.clone(),
        model: model.clone(),
        clean_make: make,
        clean_model: model,
        iso,
        shutter_speed,
        aperture,
        focal_length,
        lens_model,
        date_taken,
        file_size_bytes: file_size_bytes,
        thumbnail_base64,
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![list_arw_files, load_raw_info])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

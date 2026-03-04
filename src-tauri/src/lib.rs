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

#[derive(serde::Deserialize, Clone)]
struct DevelopParams {
    exposure: Option<f32>,       // EV: -3.0 to +3.0, default 0.0
    saturation: Option<f32>,     // 0.0 to 2.0, default 1.3
    contrast: Option<f32>,       // -1.0 to +1.0, default 0.0
    highlights: Option<f32>,     // -1.0 to +1.0, default 0.0
    shadows: Option<f32>,        // -1.0 to +1.0, default 0.0
    wb_temp_shift: Option<f32>,  // -1.0 to +1.0, default 0.0
    wb_tint_shift: Option<f32>,  // -1.0 to +1.0, default 0.0
}

/// Fully decode RAW pixel data from an ARW file.
/// Accepts optional develop parameters for exposure, saturation, contrast, etc.
#[tauri::command(async)]
fn decode_raw_full(path: String, max_dimension: Option<usize>, develop: Option<DevelopParams>) -> Result<String, String> {
    use rayon::prelude::*;

    let dev = develop.unwrap_or(DevelopParams {
        exposure: None, saturation: None, contrast: None,
        highlights: None, shadows: None, wb_temp_shift: None, wb_tint_shift: None,
    });
    let user_exposure_ev = dev.exposure.unwrap_or(0.0);
    let user_saturation = dev.saturation.unwrap_or(1.3);
    let user_contrast = dev.contrast.unwrap_or(0.0);
    let user_highlights = dev.highlights.unwrap_or(0.0);
    let user_shadows = dev.shadows.unwrap_or(0.0);
    let user_wb_temp = dev.wb_temp_shift.unwrap_or(0.0);
    let user_wb_tint = dev.wb_tint_shift.unwrap_or(0.0);

    let max_dim = max_dimension.unwrap_or(2048);

    // Decode RAW file using rawler
    let mut raw = rawler::decode_file(&path).map_err(|e| format!("RAW decode failed: {}", e))?;

    let full_w = raw.width;
    let full_h = raw.height;
    let cpp = raw.cpp;

    // Crop to active/recommended area to exclude dark sensor edge pixels (green border fix)
    let (crop_x, crop_y, crop_w, crop_h) = if let Some(ref crop) = raw.crop_area {
        (crop.p.x as usize, crop.p.y as usize, crop.d.w as usize, crop.d.h as usize)
    } else if let Some(ref active) = raw.active_area {
        (active.p.x as usize, active.p.y as usize, active.d.w as usize, active.d.h as usize)
    } else {
        (0, 0, full_w, full_h)
    };
    // Use cropped dimensions as the logical image size
    let w = crop_w;
    let h = crop_h;

    // Normalise pixel data to 0.0-1.0
    raw.apply_scaling().map_err(|e| format!("Scaling failed: {}", e))?;

    let pixels: Vec<f32> = match &raw.data {
        rawler::RawImageData::Float(data) => {
            data.iter().map(|&v| v.clamp(0.0, 1.0)).collect()
        }
        rawler::RawImageData::Integer(data) => {
            data.iter().map(|&v| (v as f32 / 65535.0).clamp(0.0, 1.0)).collect()
        }
    };

    // White balance coefficients
    let wb = raw.wb_coeffs;
    let mut wb_r = if wb[0].is_finite() && wb[0] > 0.0 { wb[0] / wb[1] } else { 1.0f32 };
    let mut wb_b = if wb[2].is_finite() && wb[2] > 0.0 { wb[2] / wb[1] } else { 1.0f32 };

    // Apply user WB adjustments: temp shifts R↔B, tint shifts G↔Mg
    // Positive temp = warmer (more red, less blue)
    wb_r *= 1.0 + user_wb_temp * 0.5;
    wb_b *= 1.0 - user_wb_temp * 0.5;
    // Tint: adjust green relative to red+blue
    let tint_factor = 1.0 + user_wb_tint * 0.3;
    wb_r *= tint_factor;
    wb_b *= tint_factor;

    // ── Auto-exposure: interquartile mean for stable brightness ──
    // Sample ~1% of pixels from the cropped active area
    let step = ((w * h) as f64).sqrt() as usize;
    let step = step.max(10);
    let mut brightness_samples: Vec<f32> = Vec::with_capacity(w * h / (step * step) + 1);
    {
        let mut row = 0;
        while row < h {
            let mut col = 0;
            while col < w {
                let sr = row + crop_y;
                let sc = col + crop_x;
                let val = pixels[sr * full_w * cpp.max(1) + sc * cpp.max(1)];
                brightness_samples.push(val);
                col += step;
            }
            row += step;
        }
    }
    brightness_samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    // Interquartile mean: average of the middle 50% (P25-P75)
    // This ignores extreme shadows and blown highlights for a stable estimate
    let n = brightness_samples.len();
    let q1 = n / 4;
    let q3 = 3 * n / 4;
    let iqm = if q3 > q1 {
        let sum: f32 = brightness_samples[q1..q3].iter().sum();
        sum / (q3 - q1) as f32
    } else {
        brightness_samples.get(n / 2).copied().unwrap_or(0.18)
    };

    // Target: push interquartile mean to linear middle gray (0.18)
    // This is the standard photographic midpoint and gives consistent results
    let exposure_mult = if iqm > 0.005 {
        (0.18 / iqm).clamp(0.5, 4.0) // allow slight darkening too, max +2EV
    } else {
        2.0 // fallback for extremely dark images
    };

    // CFA pattern
    let cfa = raw.camera.cfa.clone();

    // Output dimensions
    let scale = if w.max(h) > max_dim {
        max_dim as f32 / w.max(h) as f32
    } else {
        1.0f32
    };
    let out_w = (w as f32 * scale) as usize;
    let out_h = (h as f32 * scale) as usize;

    // Block size for area-averaged downscaling (acts as noise reduction)
    let block = (1.0 / scale).round().max(1.0) as usize;

    // sRGB gamma LUT
    let gamma_lut: Vec<u8> = (0..4096).map(|i| {
        let v = i as f32 / 4095.0;
        let g = if v <= 0.0031308 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (g.clamp(0.0, 1.0) * 255.0) as u8
    }).collect();

    let apply_gamma = |v: f32| -> u8 {
        let idx = (v.clamp(0.0, 1.0) * 4095.0) as usize;
        gamma_lut[idx.min(4095)]
    };

    // Area-averaged demosaic: average a block of source pixels per color channel
    // All coordinates are in cropped space; offset by crop_x/crop_y for raw pixel access
    let sample_block = |cy: usize, cx: usize, target_c: usize| -> f32 {
        let y_start = cy.saturating_sub(block / 2);
        let y_end = (cy + block / 2 + 1).min(h);
        let x_start = cx.saturating_sub(block / 2);
        let x_end = (cx + block / 2 + 1).min(w);

        let mut sum = 0.0f32;
        let mut count = 0u32;

        if cpp != 1 {
            for row in y_start..y_end {
                for col in x_start..x_end {
                    let sr = row + crop_y;
                    let sc = col + crop_x;
                    sum += pixels[sr * full_w * cpp + sc * cpp + target_c];
                    count += 1;
                }
            }
        } else {
            for row in y_start..y_end {
                for col in x_start..x_end {
                    let sr = row + crop_y;
                    let sc = col + crop_x;
                    if cfa.color_at(sr, sc) == target_c {
                        sum += pixels[sr * full_w + sc];
                        count += 1;
                    }
                }
            }
        }

        if count > 0 { sum / count as f32 } else { 0.0 }
    };

    // Parallel output: process rows in parallel using rayon
    let mut output_bytes = vec![0u8; out_w * out_h * 3];

    output_bytes
        .par_chunks_mut(out_w * 3)
        .enumerate()
        .for_each(|(oy, row_buf)| {
            let sy = ((oy as f32 / scale) as usize).min(h - 1);
            for ox in 0..out_w {
                let sx = ((ox as f32 / scale) as usize).min(w - 1);

                // Area-averaged demosaic + WB + auto-exposure + manual exposure
                let user_exp_mult = 2.0f32.powf(user_exposure_ev);
                let total_exp = exposure_mult * user_exp_mult;
                let mut r = (sample_block(sy, sx, 0) * wb_r * total_exp).clamp(0.0, 1.0);
                let mut g = (sample_block(sy, sx, 1) * total_exp).clamp(0.0, 1.0);
                let mut b = (sample_block(sy, sx, 2) * wb_b * total_exp).clamp(0.0, 1.0);

                // Highlights / Shadows adjustment (tone separation in linear space)
                let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                if user_highlights != 0.0 && lum > 0.5 {
                    // Pull highlights: negative = recover, positive = brighten
                    let strength = (lum - 0.5) * 2.0 * user_highlights * 0.5;
                    r = (r - strength).clamp(0.0, 1.0);
                    g = (g - strength).clamp(0.0, 1.0);
                    b = (b - strength).clamp(0.0, 1.0);
                }
                if user_shadows != 0.0 && lum < 0.5 {
                    // Lift shadows: positive = brighten shadows
                    let strength = (0.5 - lum) * 2.0 * user_shadows * 0.4;
                    r = (r + strength).clamp(0.0, 1.0);
                    g = (g + strength).clamp(0.0, 1.0);
                    b = (b + strength).clamp(0.0, 1.0);
                }

                // Contrast: S-curve in linear space
                if user_contrast != 0.0 {
                    let c = user_contrast * 0.5; // scale to reasonable range
                    let apply_contrast = |v: f32| -> f32 {
                        // Simple S-curve: shift midpoint and steepen
                        let centered = v - 0.5;
                        let curved = centered * (1.0 + c * 2.0);
                        (curved + 0.5).clamp(0.0, 1.0)
                    };
                    r = apply_contrast(r);
                    g = apply_contrast(g);
                    b = apply_contrast(b);
                }

                // Saturation
                let lum2 = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                r = (lum2 + (r - lum2) * user_saturation).clamp(0.0, 1.0);
                g = (lum2 + (g - lum2) * user_saturation).clamp(0.0, 1.0);
                b = (lum2 + (b - lum2) * user_saturation).clamp(0.0, 1.0);

                row_buf[ox * 3]     = apply_gamma(r);
                row_buf[ox * 3 + 1] = apply_gamma(g);
                row_buf[ox * 3 + 2] = apply_gamma(b);
            }
        });

    // Create image from raw bytes
    let img_buf = image::RgbImage::from_raw(out_w as u32, out_h as u32, output_bytes)
        .ok_or("Failed to create image buffer")?;

    // Encode as JPEG
    let mut jpeg_buf = std::io::Cursor::new(Vec::new());
    img_buf
        .write_to(&mut jpeg_buf, image::ImageFormat::Jpeg)
        .map_err(|e| format!("JPEG encode failed: {}", e))?;

    let b64 = general_purpose::STANDARD.encode(jpeg_buf.into_inner());
    Ok(format!("data:image/jpeg;base64,{}", b64))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![list_arw_files, load_raw_info, decode_raw_full])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

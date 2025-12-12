use leptess::{LepTess, Variable};
use image::{DynamicImage, ImageFormat};
use std::io::Cursor;
use log;

pub fn perform_ocr(img_buffer: &DynamicImage) -> Result<String, String> {
    log::info!("Starting OCR process with leptess");

    // Convert the image to a PNG byte vector
    let mut img_bytes: Vec<u8> = Vec::new();
    img_buffer
        .write_to(&mut Cursor::new(&mut img_bytes), ImageFormat::Png)
        .map_err(|e| format!("Failed to convert image to PNG: {}", e))?;

    // Initialize Tesseract with leptess
    let mut lt = LepTess::new(None, "eng").map_err(|e| format!("Failed to initialize Tesseract: {}", e))?;

    // Set Tesseract parameters (whitelist)
    if let Err(e) = lt.set_variable(Variable::TesseditCharWhitelist, "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ!\"#$%&'()*+,-./:;<=>?@[]^_`{|}~ ") {
        log::warn!("Failed to set Tesseract character whitelist: {}", e);
    }

    // Set the image from memory
    lt.set_image_from_mem(&img_bytes).map_err(|e| format!("Failed to set image for OCR: {}", e))?;

    // Perform OCR
    let text = lt.get_utf8_text().map_err(|e| format!("OCR failed: {}", e))?;

    log::info!("OCR successful. Text found (len: {})", text.len());

    Ok(text)
}

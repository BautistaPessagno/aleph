use std::fs;
use std::path::Path;
use icns::{IconFamily, IconType};
use image::{ImageFormat, DynamicImage};
use base64::{Engine as _, engine::general_purpose};

/// Extract app icon from macOS .app bundle
pub fn extract_app_icon(app_path: &str) -> Option<String> {
    // Path to the icon file inside the .app bundle
    let icon_path = Path::new(app_path)
        .join("Contents")
        .join("Resources")
        .join("AppIcon.icns");
    
    let final_icon_path = if !icon_path.exists() {
        // Try alternative icon names
        let alternative_paths = [
            Path::new(app_path).join("Contents").join("Resources").join("app.icns"),
            Path::new(app_path).join("Contents").join("Resources").join("icon.icns"),
            Path::new(app_path).join("Contents").join("Resources").join("AppIcon.icns"),
        ];
        
        let mut found_path = None;
        for alt_path in &alternative_paths {
            if alt_path.exists() {
                found_path = Some(alt_path.clone());
                break;
            }
        }
        
        if found_path.is_none() {
            // Try to find any .icns file in Resources directory
            let resources_dir = Path::new(app_path).join("Contents").join("Resources");
            if let Ok(entries) = fs::read_dir(resources_dir) {
                for entry in entries.flatten() {
                    if let Some(ext) = entry.path().extension() {
                        if ext == "icns" {
                            found_path = Some(entry.path());
                            break;
                        }
                    }
                }
            }
        }
        
        match found_path {
            Some(path) => path,
            None => return None,
        }
    } else {
        icon_path.to_path_buf()
    };
    
    // Read and parse the ICNS file
    let icon_data = fs::read(&final_icon_path).ok()?;
    let icon_family = IconFamily::read(std::io::Cursor::new(icon_data)).ok()?;
    
    // Try to get the best quality icon (128x128 or 64x64)
    let icon_types = [
        IconType::RGBA32_128x128,    // 128x128
        IconType::RGBA32_64x64,      // 64x64
        IconType::RGBA32_32x32,      // 32x32
        IconType::RGBA32_16x16,      // 16x16
    ];
    
    for icon_type in &icon_types {
        if let Ok(icon) = icon_family.get_icon_with_type(*icon_type) {
            // Convert to PNG and encode as base64
            let rgba_data = icon.data();
            let width = icon.width();
            let height = icon.height();
            
            if let Some(img) = image::RgbaImage::from_raw(width, height, rgba_data.to_vec()) {
                let dynamic_img = DynamicImage::ImageRgba8(img);
                let mut png_data = Vec::new();
                if dynamic_img.write_to(&mut std::io::Cursor::new(&mut png_data), ImageFormat::Png).is_ok() {
                    let base64_string = general_purpose::STANDARD.encode(png_data);
                    return Some(format!("data:image/png;base64,{}", base64_string));
                }
            }
        }
    }
    
    None
}

/// Get file icon based on extension
pub fn get_file_icon(_file_path: &str, extension: &str) -> Option<String> {
    // For now, we'll use a simple mapping based on extensions
    // In the future, this could be enhanced to use system APIs to get actual file icons
    
    match extension.to_lowercase().as_str() {
        // Images
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "tiff" | "webp" | "svg" => {
            Some("data:image/svg+xml;base64,PHN2ZyB3aWR0aD0iMzIiIGhlaWdodD0iMzIiIHZpZXdCb3g9IjAgMCAzMiAzMiIgZmlsbD0ibm9uZSIgeG1sbnM9Imh0dHA6Ly93d3cudzMub3JnLzIwMDAvc3ZnIj4KPHJlY3Qgd2lkdGg9IjMyIiBoZWlnaHQ9IjMyIiByeD0iNCIgZmlsbD0iIzRGNDZFNSIvPgo8cGF0aCBkPSJNOCA4SDE2VjEwSDhWOFoiIGZpbGw9IndoaXRlIi8+CjxwYXRoIGQ9Ik04IDEySDI0VjE0SDhWMTJaIiBmaWxsPSJ3aGl0ZSIvPgo8cGF0aCBkPSJNOCAxNkgyNFYxOEg4VjE2WiIgZmlsbD0id2hpdGUiLz4KPHBhdGggZD0iTTggMjBIMTZWMjJIOFYyMFoiIGZpbGw9IndoaXRlIi8+Cjwvc3ZnPgo=".to_string())
        },
        
        // Videos
        "mp4" | "mov" | "avi" | "mkv" | "wmv" | "flv" | "webm" => {
            Some("data:image/svg+xml;base64,PHN2ZyB3aWR0aD0iMzIiIGhlaWdodD0iMzIiIHZpZXdCb3g9IjAgMCAzMiAzMiIgZmlsbD0ibm9uZSIgeG1sbnM9Imh0dHA6Ly93d3cudzMub3JnLzIwMDAvc3ZnIj4KPHJlY3Qgd2lkdGg9IjMyIiBoZWlnaHQ9IjMyIiByeD0iNCIgZmlsbD0iI0ZGNkI2QiIvPgo8cGF0aCBkPSJNMTIgMTBMMjAgMTZMMTIgMjJWMTBaIiBmaWxsPSJ3aGl0ZSIvPgo8L3N2Zz4K".to_string())
        },
        
        // Audio
        "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" => {
            Some("data:image/svg+xml;base64,PHN2ZyB3aWR0aD0iMzIiIGhlaWdodD0iMzIiIHZpZXdCb3g9IjAgMCAzMiAzMiIgZmlsbD0ibm9uZSIgeG1sbnM9Imh0dHA6Ly93d3cudzMub3JnLzIwMDAvc3ZnIj4KPHJlY3Qgd2lkdGg9IjMyIiBoZWlnaHQ9IjMyIiByeD0iNCIgZmlsbD0iIzFEQjk1NCIvPgo8cGF0aCBkPSJNMTYgOEMxMC40NzcgOCA2IDEyLjQ3NyA2IDE4UzEwLjQ3NyAyOCAxNiAyOFMyNiAyMy41MjMgMjYgMThTMjEuNTIzIDggMTYgOFpNMTYgMjJDMTMuNzkxIDIyIDEyIDIwLjIwOSAxMiAxOFMxMy43OTEgMTQgMTYgMTRTMjAgMTUuNzkxIDIwIDE4UzE4LjIwOSAyMiAxNiAyMloiIGZpbGw9IndoaXRlIi8+Cjwvc3ZnPgo=".to_string())
        },
        
        // Archives
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" => {
            Some("data:image/svg+xml;base64,PHN2ZyB3aWR0aD0iMzIiIGhlaWdodD0iMzIiIHZpZXdCb3g9IjAgMCAzMiAzMiIgZmlsbD0ibm9uZSIgeG1sbnM9Imh0dHA6Ly93d3cudzMub3JnLzIwMDAvc3ZnIj4KPHJlY3Qgd2lkdGg9IjMyIiBoZWlnaHQ9IjMyIiByeD0iNCIgZmlsbD0iI0ZGOTUwMCIvPgo8cGF0aCBkPSJNOCAxMkg4LjVDOS4zMjg0MyAxMiAxMCAxMi42NzE2IDEwIDEzLjVWMTguNUMxMCAxOS4zMjg0IDkuMzI4NDMgMjAgOC5IDIwSDhWMTJaIiBmaWxsPSJ3aGl0ZSIvPgo8cGF0aCBkPSJNMTQgMTJIMTQuNUMxNS4zMjg0IDEyIDE2IDEyLjY3MTYgMTYgMTMuNVYxOC41QzE2IDE5LjMyODQgMTUuMzI4NCAyMCAxNC41IDIwSDE0VjEyWiIgZmlsbD0id2hpdGUiLz4KPHN2Zz4K".to_string())
        },
        
        // Code files
        "js" | "ts" | "jsx" | "tsx" | "py" | "java" | "cpp" | "c" | "h" | "rs" | "go" | "php" | "rb" | "swift" => {
            Some("data:image/svg+xml;base64,PHN2ZyB3aWR0aD0iMzIiIGhlaWdodD0iMzIiIHZpZXdCb3g9IjAgMCAzMiAzMiIgZmlsbD0ibm9uZSIgeG1sbnM9Imh0dHA6Ly93d3cudzMub3JnLzIwMDAvc3ZnIj4KPHJlY3Qgd2lkdGg9IjMyIiBoZWlnaHQ9IjMyIiByeD0iNCIgZmlsbD0iIzA3N0RGRiIvPgo8cGF0aCBkPSJNMTAgMTJMMTQgMTZMMTAgMjAiIHN0cm9rZT0id2hpdGUiIHN0cm9rZS13aWR0aD0iMiIgc3Ryb2tlLWxpbmVjYXA9InJvdW5kIiBzdHJva2UtbGluZWpvaW49InJvdW5kIi8+CjxwYXRoIGQ9Ik0xNiAyMEgyMiIgc3Ryb2tlPSJ3aGl0ZSIgc3Ryb2tlLXdpZHRoPSIyIiBzdHJva2UtbGluZWNhcD0icm91bmQiLz4KPC9zdmc+Cg==".to_string())
        },
        
        // Documents
        "pdf" => {
            Some("data:image/svg+xml;base64,PHN2ZyB3aWR0aD0iMzIiIGhlaWdodD0iMzIiIHZpZXdCb3g9IjAgMCAzMiAzMiIgZmlsbD0ibm9uZSIgeG1sbnM9Imh0dHA6Ly93d3cudzMub3JnLzIwMDAvc3ZnIj4KPHJlY3Qgd2lkdGg9IjMyIiBoZWlnaHQ9IjMyIiByeD0iNCIgZmlsbD0iI0RDMjYyNiIvPgo8dGV4dCB4PSI1IiB5PSIyMCIgZm9udC1mYW1pbHk9IkFyaWFsLCBzYW5zLXNlcmlmIiBmb250LXNpemU9IjEwIiBmb250LXdlaWdodD0iYm9sZCIgZmlsbD0id2hpdGUiPlBERjwvdGV4dD4KPC9zdmc+Cg==".to_string())
        },
        
        "txt" | "md" | "rtf" => {
            Some("data:image/svg+xml;base64,PHN2ZyB3aWR0aD0iMzIiIGhlaWdodD0iMzIiIHZpZXdCb3g9IjAgMCAzMiAzMiIgZmlsbD0ibm9uZSIgeG1sbnM9Imh0dHA6Ly93d3cudzMub3JnLzIwMDAvc3ZnIj4KPHJlY3Qgd2lkdGg9IjMyIiBoZWlnaHQ9IjMyIiByeD0iNCIgZmlsbD0iIzY1NjU2NSIvPgo8cGF0aCBkPSJNOCA4SDE2VjEwSDhWOFoiIGZpbGw9IndoaXRlIi8+CjxwYXRoIGQ9Ik04IDEySDI0VjE0SDhWMTJaIiBmaWxsPSJ3aGl0ZSIvPgo8cGF0aCBkPSJNOCAxNkgyNFYxOEg4VjE2WiIgZmlsbD0id2hpdGUiLz4KPHBhdGggZD0iTTggMjBIMTZWMjJIOFYyMFoiIGZpbGw9IndoaXRlIi8+Cjwvc3ZnPgo=".to_string())
        },
        
        // Default file icon
        _ => {
            Some("data:image/svg+xml;base64,PHN2ZyB3aWR0aD0iMzIiIGhlaWdodD0iMzIiIHZpZXdCb3g9IjAgMCAzMiAzMiIgZmlsbD0ibm9uZSIgeG1sbnM9Imh0dHA6Ly93d3cudzMub3JnLzIwMDAvc3ZnIj4KPHJlY3Qgd2lkdGg9IjMyIiBoZWlnaHQ9IjMyIiByeD0iNCIgZmlsbD0iIzY1NjU2NSIvPgo8cGF0aCBkPSJNMTYgOEwxOCAxMEgyNlYyNkg2VjEwSDE0TDE2IDhaIiBmaWxsPSJ3aGl0ZSIvPgo8L3N2Zz4K".to_string())
        }
    }
}

/// Check if a file is an executable application
pub fn is_executable(path: &str) -> bool {
    let path = Path::new(path);
    
    // Check if it's a macOS app bundle
    if let Some(extension) = path.extension() {
        if extension == "app" {
            return true;
        }
    }
    
    // Check if it has executable permissions (Unix-like systems)
    if let Ok(metadata) = std::fs::metadata(path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = metadata.permissions();
            return permissions.mode() & 0o111 != 0;
        }
    }
    
    false
}
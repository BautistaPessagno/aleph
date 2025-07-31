// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/

//manejo de paths y archivos
use jwalk::rayon::iter::{ParallelBridge, ParallelIterator};
use std::path::Path;
use std::{fs, path::PathBuf};
//tantivy
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{doc, Index, IndexWriter};
use tantivy::TantivyError;
//WalkDir
use jwalk::{Parallelism, WalkDir};
//opener
use opener;
//dir
use dirs;
//tokio
use tokio;
use std::sync::LazyLock;
//icon extraction
use image::{ImageFormat, DynamicImage, GenericImage};
use base64::{Engine as _, engine::general_purpose};
use sha2::{Sha256, Digest};
use std::io::Cursor;

// Runtime estático reutilizable
static RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime")
});

fn get_home_child_folders() -> Result<Vec<PathBuf>, String> {
    let home = dirs::home_dir().ok_or("Could not get home directory")?;
    
    let mut folders = Vec::new();
    
    for entry in fs::read_dir(&home).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        
        if path.is_dir() {
            // Skip Applications folder
            if let Some(folder_name) = path.file_name() {
                if folder_name != "Applications" {
                    folders.push(path);
                }
            }
        }
    }
    
    Ok(folders)
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

async fn create_index(root_dir: &Path) -> Result<(), String> {
    //El index se va a guardar en ~/.cache/aleph/index/{folder_name}
    //Si no existe el path se crea
    let home = dirs::home_dir().unwrap();
    let folder_name = root_dir.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    let idx_path = home.join(".cache/aleph/index").join(folder_name);
    let idx_path = idx_path.as_path();

    if !idx_path.exists() {
        fs::create_dir_all(idx_path).map_err(|e| e.to_string())?;
    }

    let mut schema_builder = Schema::builder();

    schema_builder.add_text_field("path", STORED | STRING);
    schema_builder.add_text_field("filename", TEXT | STORED);
    schema_builder.add_text_field("extension", STRING | STORED);

    let schema = schema_builder.build();

    //  Abrir o crear el índice de forma segura
    let index: Index = match Index::create_in_dir(idx_path, schema.clone()) {
        Ok(idx) => idx, // creado de cero
        Err(TantivyError::IndexAlreadyExists) => {
            // fs::remove_file(idx_path.join("meta.json")).map_err(|e| e.to_string())?;
            // Index::create_in_dir(idx_path, schema.clone()).map_err(|e| e.to_string())?
            //si lo encuentra lo abre
            Index::open_in_dir(idx_path).map_err(|e| e.to_string())?
        } // ya existía
        Err(e) => return Err(e.to_string()), // otro error
    };

    let mut index_writer: IndexWriter = index
        .writer_with_num_threads(10, 200_000_000)
        .map_err(|e| e.to_string())?;
    //B: let writer = Arc::new(index.writer(50_000_000)?);

    let path_f = schema.get_field("path").unwrap();
    let filename = schema.get_field("filename").unwrap();
    let ext_f = schema.get_field("extension").unwrap();

    //Vamos a indexar todo
    WalkDir::new(root_dir)
        .skip_hidden(true)
        .follow_links(true)
        .parallelism(Parallelism::RayonNewPool(8))
        .into_iter()
        .par_bridge()
        .for_each(|res| {
            if let Ok(entry) = res {
                // Skip hidden files and directories
                if let Some(file_name) = entry.file_name().to_str() {
                    if file_name.starts_with('.') {
                        return;
                    }
                }
                
                // Skip temporary and build directories
                if entry.file_type().is_dir() {
                    if let Some(dir_name) = entry.file_name().to_str() {
                        let skip_dirs = [
                            "node_modules", "target", "build", "dist", "tmp", "temp",
                            ".git", ".svn", ".hg", "__pycache__", ".cache", ".vscode",
                            ".idea", "coverage", ".nyc_output", "logs", "log"
                        ];
                        if skip_dirs.contains(&dir_name) {
                            return;
                        }
                    }
                }
                
                //filtro si no es un directorio
                if entry.file_type().is_file() {
                    // Skip temporary file extensions
                    let ext = entry
                        .path()
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    
                    let skip_extensions = [
                        "tmp", "temp", "bak", "swp", "swo", "log", "cache", 
                        "lock", "pid", "DS_Store"
                    ];
                    
                    if skip_extensions.contains(&ext.as_str()) {
                        return;
                    }
                    
                    // Skip files with temporary patterns
                    let file_name = entry.file_name().to_string_lossy();
                    if file_name.ends_with('~') || file_name.starts_with('#') || file_name.ends_with('#') {
                        return;
                    }
                    
                    let path = entry.path().display().to_string();
                    let name = file_name.to_string();
                    
                    let doc = doc!(
                        path_f => path,
                        filename => name.as_str(),
                        ext_f => ext.as_str(),
                    );
                    index_writer.add_document(doc).unwrap();
                }
            }
        });
    index_writer.commit().map_err(|e| e.to_string())?;
    index_writer
        .wait_merging_threads()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn search_index(query: &str) -> Result<Vec<(String, String)>, String> {
    let home = dirs::home_dir().unwrap();
    
    // Función auxiliar para buscar en un índice específico
    fn search_in_index(idx_path: &Path, query: &str, limit: usize) -> Vec<(String, String)> {
        let mut results = Vec::new();
        
        if let Ok(index) = Index::open_in_dir(idx_path) {
            if let Ok(reader) = index.reader() {
                let schema = index.schema();
                
                let path_f = schema.get_field("path").unwrap();
                let filename = schema.get_field("filename").unwrap();
                let ext_f = schema.get_field("extension").unwrap();
                
                let searcher = reader.searcher();
                
                let mut query_parser = QueryParser::for_index(&index, vec![path_f, filename, ext_f]);
                query_parser.set_field_fuzzy(filename, false, 2, true);
                
                if let Ok(parsed_query) = query_parser.parse_query(query) {
                    if let Ok(top_docs) = searcher.search(&parsed_query, &TopDocs::with_limit(limit)) {
                        for (_score, doc_address) in top_docs {
                            if let Ok(retrieved_doc) = searcher.doc(doc_address) {
                                let retrieved_doc: TantivyDocument = retrieved_doc;
                                let name = retrieved_doc
                                    .get_first(filename)
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_owned();
                                
                                let path = retrieved_doc
                                    .get_first(path_f)
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_owned();
                                
                                results.push((name, path));
                            }
                        }
                    }
                }
            }
        }
        results
    }
    
    // Buscar primero en Desktop (más rápido y relevante)
    let desktop_idx_path = home.join(".cache/aleph/index/Desktop");
    
    // Crear índice de Desktop si no existe
    if !desktop_idx_path.exists() {
        let desktop_path = home.join("Desktop");
        RUNTIME.block_on(async {
            create_index(&desktop_path).await.map_err(|e| e.to_string())
        })?;
    }
    
    // Buscar en Desktop primero
    let mut results = search_in_index(&desktop_idx_path, query, 15);
    
    // Si no hay suficientes resultados, buscar en otras carpetas principales
    if results.len() < 5 {
        let priority_folders = ["Documents", "Downloads", "Pictures", "Music", "Videos"];
        
        for folder_name in &priority_folders {
            let idx_path = home.join(".cache/aleph/index").join(folder_name);
            let root_path = home.join(folder_name);
            
            // Crear índice en background si no existe (no bloquear)
            if !idx_path.exists() && root_path.exists() {
                let root_path_clone = root_path.clone();
                RUNTIME.spawn(async move {
                    let _ = create_index(&root_path_clone).await;
                });
            } else if idx_path.exists() {
                // Solo buscar si el índice ya existe
                let folder_results = search_in_index(&idx_path, query, 5);
                results.extend(folder_results);
                
                if results.len() >= 10 {
                    break;
                }
            }
        }
    }
    
    // Crear índices para otras carpetas en background (no bloquear la búsqueda)
    RUNTIME.spawn(async move {
        if let Ok(root_folders) = get_home_child_folders() {
            for root_folder in root_folders {
                let folder_name = root_folder.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown");
                let idx_path = home.join(".cache/aleph/index").join(folder_name);
                
                if !idx_path.exists() && !["Desktop", "Documents", "Downloads", "Pictures", "Music", "Videos"].contains(&folder_name) {
                    let _ = create_index(&root_folder).await;
                }
            }
        }
    });
    
    // Limitar resultados y eliminar duplicados
    results.sort_by(|a, b| a.0.cmp(&b.0));
    results.dedup();
    results.truncate(10);
    
    Ok(results)
}

async fn create_app_launcher() -> Result<(), String> {
    //El index se va a guardar en ~/.cache/aleph/index
    //Si no existe el path se crea
    let home = dirs::home_dir().unwrap();
    let idx_path = home.join(".cache/aleph/apps");
    let idx_path = idx_path.as_path();

    if !idx_path.exists() {
        fs::create_dir_all(idx_path).map_err(|e| e.to_string())?;
    }

    // Create icon cache directory
    let icon_cache_path = home.join(".cache/aleph/icons");
    if !icon_cache_path.exists() {
        fs::create_dir_all(&icon_cache_path).map_err(|e| e.to_string())?;
    }

    let mut schema_builder = Schema::builder();

    schema_builder.add_text_field("path", STORED | STRING);
    schema_builder.add_text_field("filename", TEXT | STORED);
    schema_builder.add_text_field("extension", STRING | STORED);
    schema_builder.add_text_field("icon_path", STORED | STRING);
    schema_builder.add_text_field("icon_base64", STORED | STRING);

    let schema = schema_builder.build();

    //  Abrir o crear el índice de forma segura
    let index: Index = match Index::create_in_dir(idx_path, schema.clone()) {
        Ok(idx) => idx, // creado de cero
        Err(_) => {
            // fs::remove_file(idx_path.join("meta.json")).map_err(|e| e.to_string())?;
            // Index::create_in_dir(idx_path, schema.clone()).map_err(|e| e.to_string())?
            //si lo encuentra lo abre
            Index::open_in_dir(idx_path).map_err(|e| e.to_string())?
        } // ya existía
    };

    let mut index_writer: IndexWriter = index
        .writer_with_num_threads(10, 200_000_000)
        .map_err(|e| e.to_string())?;
    //B: let writer = Arc::new(index.writer(50_000_000)?);

    //voy mandando el indeice de a partes para que se pueda busar aunque el indice no este completo

    let path_f = schema.get_field("path").unwrap();
    let filename = schema.get_field("filename").unwrap();
    let ext_f = schema.get_field("extension").unwrap();
    let _icon_path_f = schema.get_field("icon_path").unwrap();
    let _icon_base64_f = schema.get_field("icon_base64").unwrap();

    //Vamos a indexar todo
    let root_dir = Path::new("/Applications");

    WalkDir::new(root_dir)
        .max_depth(1)
        .skip_hidden(true)
        .follow_links(true)
        .parallelism(Parallelism::RayonNewPool(8))
        .into_iter()
        .par_bridge()
        .for_each(|res| {
            if let Ok(entry) = res {
                //Filtro si es un .app
                if entry
                    .path()
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_lowercase()
                    == "app"
                {
                    let path = entry.path().display().to_string();
                    let name = entry.file_name().to_string_lossy();
                    let ext = entry
                        .path()
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    let doc = doc!(
                        path_f => path,
                        filename => name.as_ref(),
                        ext_f => ext.as_str(),
                    );
                    index_writer.add_document(doc).unwrap();
                }
            }
        });
    index_writer.commit().map_err(|e| e.to_string())?;
    index_writer
        .wait_merging_threads()
        .map_err(|e| e.to_string())?;

    Ok(())
}

fn extract_app_icon(app_path: &Path, icon_cache_path: &Path) -> Result<(String, String), String> {
    // Generate a hash for the app path to create a unique filename
    let mut hasher = Sha256::new();
    hasher.update(app_path.to_string_lossy().as_bytes());
    let hash = format!("{:x}", hasher.finalize());
    let icon_filename = format!("{}.png", &hash[..16]);
    let icon_file_path = icon_cache_path.join(&icon_filename);
    
    // If icon already exists in cache, return it
    if icon_file_path.exists() {
        if let Ok(icon_data) = fs::read(&icon_file_path) {
            let base64_icon = general_purpose::STANDARD.encode(&icon_data);
            return Ok((icon_file_path.to_string_lossy().to_string(), base64_icon));
        }
    }
    
    // Try to extract icon from macOS app bundle
    let icon_path = app_path.join("Contents/Resources");
    
    if icon_path.exists() {
        // Look for icns files
        if let Ok(entries) = fs::read_dir(&icon_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(extension) = path.extension() {
                    if extension == "icns" {
                        // For now, we'll create a placeholder icon
                        // In a full implementation, you'd parse the ICNS file
                        if let Ok(placeholder_icon) = create_placeholder_icon(&app_path.file_name().unwrap_or_default().to_string_lossy()) {
                            if let Ok(_) = fs::write(&icon_file_path, &placeholder_icon) {
                                let base64_icon = general_purpose::STANDARD.encode(&placeholder_icon);
                                return Ok((icon_file_path.to_string_lossy().to_string(), base64_icon));
                            }
                        }
                        break;
                    }
                }
            }
        }
    }
    
    // Fallback: create a simple placeholder icon
    if let Ok(placeholder_icon) = create_placeholder_icon(&app_path.file_name().unwrap_or_default().to_string_lossy()) {
        if let Ok(_) = fs::write(&icon_file_path, &placeholder_icon) {
            let base64_icon = general_purpose::STANDARD.encode(&placeholder_icon);
            return Ok((icon_file_path.to_string_lossy().to_string(), base64_icon));
        }
    }
    
    Err("Could not extract or create icon".to_string())
}

fn create_placeholder_icon(app_name: &str) -> Result<Vec<u8>, String> {
    // Create a simple 64x64 colored square as placeholder
    let mut img = DynamicImage::new_rgb8(64, 64);
    
    // Use the first character of the app name to determine color
    let first_char = app_name.chars().next().unwrap_or('A') as u8;
    let color_r = (first_char.wrapping_mul(67)) % 200 + 55;
    let color_g = (first_char.wrapping_mul(113)) % 200 + 55;
    let color_b = (first_char.wrapping_mul(179)) % 200 + 55;
    
    // Fill the image with the color
    for x in 0..64 {
        for y in 0..64 {
            img.put_pixel(x, y, image::Rgba([color_r, color_g, color_b, 255]));
        }
    }
    
    // Convert to PNG bytes
    let mut buffer = Vec::new();
    let mut cursor = Cursor::new(&mut buffer);
    img.write_to(&mut cursor, ImageFormat::Png).map_err(|e| e.to_string())?;
    
    Ok(buffer)
}

#[tauri::command]
fn app_search(query: &str) -> Result<Vec<(String, String)>, String> {
    //Deberia chequear si se creo el index
    let home = dirs::home_dir().unwrap();
    let idx_path = home.join(".cache/aleph/apps");
    let idx_path = idx_path.as_path();

    if !idx_path.exists() {
        fs::create_dir_all(idx_path).map_err(|e| e.to_string())?;
    }

    //  Abrir o crear el índice de forma segura
    let index = match Index::open_in_dir(idx_path) {
        Ok(idx) => idx, // lo encuentra, lo habre
        Err(_) => {
            // Índice no existe o hay otro error
            // Crear el índice y luego abrirlo
            RUNTIME.block_on(async { create_app_launcher().await.map_err(|e| e.to_string()) })?;
            Index::open_in_dir(idx_path).map_err(|e| e.to_string())?
        }
    };

    let reader = index.reader().map_err(|e| e.to_string())?;

    let schema = index.schema();

    let path_f = schema.get_field("path").unwrap();
    let filename = schema.get_field("filename").unwrap();
    let ext_f = schema.get_field("extension").unwrap();

    let searcher = reader.searcher();

    let mut query_parser = QueryParser::for_index(&index, vec![path_f, filename, ext_f]);
    query_parser.set_field_fuzzy(filename, false, 2, true);

    let query = query_parser.parse_query(query).map_err(|e| e.to_string())?;

    let top_docs = searcher
        .search(&query, &TopDocs::with_limit(10))
        .map_err(|e| e.to_string())?;

    let mut top_docs_vec: Vec<(String, String)> = Vec::with_capacity(top_docs.len());

    for (_score, doc_address) in top_docs {
        let retrieved_doc: TantivyDocument =
            searcher.doc(doc_address).map_err(|e| e.to_string())?;
        let name = retrieved_doc
            .get_first(filename)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();

        let path = retrieved_doc
            .get_first(path_f)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();

        top_docs_vec.push((name, path));
    }
    Ok(top_docs_vec)
}

#[tauri::command]
fn get_app_icon(app_path: &str) -> Result<String, String> {
    let home = dirs::home_dir().ok_or("Could not get home directory")?;
    let icon_cache_path = home.join(".cache/aleph/icons");
    
    if !icon_cache_path.exists() {
        fs::create_dir_all(&icon_cache_path).map_err(|e| e.to_string())?;
    }
    
    let app_path_buf = PathBuf::from(app_path);
    match extract_app_icon(&app_path_buf, &icon_cache_path) {
        Ok((_, base64_icon)) => Ok(base64_icon),
        Err(e) => {
            // Return a default icon as base64
            if let Ok(placeholder_icon) = create_placeholder_icon(&app_path_buf.file_name().unwrap_or_default().to_string_lossy()) {
                let base64_icon = general_purpose::STANDARD.encode(&placeholder_icon);
                Ok(base64_icon)
            } else {
                Err(e)
            }
        }
    }
}


#[tauri::command]
fn open_path(path: &str) -> Result<(), String> {
    opener::open(path).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            search_index,
            open_path,
            app_search,
            get_app_icon
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_test() {
        //Primero me fijo de que se cree el index
        let home = dirs::home_dir().unwrap();
        let desktop_path = home.join("Desktop");

        let ans = RUNTIME.block_on(async { create_index(&desktop_path).await });

        match ans {
            Ok(_e) => println!("el index no paniqueo"),
            Err(e) => eprint!("hay algun error al crear el index {:?}", e),
        }
        assert!(
            fs::exists("/Users/bautistapessagno/.cache/aleph/index/Desktop/meta.json").unwrap_or(false)
        );

        //creo bien el index, pero encuentra cosas?
        let search = match search_index("leetcode.c") {
            Ok(top) => top,
            Err(e) => panic!("Error al buscar: {:?}", e),
        };
        //si llegamos hasta aca no hay errores, falta ver que busque bien
        // assert!(!search.is_empty());

        assert!(search.contains(&(
            "leetcode.c".to_string(),
            "/Users/bautistapessagno/Desktop/leetcode.c".to_string()
        )));
    }

    #[test]
    fn test_apps() {
        //Primero me fijo de que se cree el index
        let ans = RUNTIME.block_on(async { create_app_launcher().await });
        match ans {
            Ok(_e) => println!("el index no paniqueo"),
            Err(e) => eprint!("hay algun error al crear el index {:?}", e),
        }
        assert!(fs::exists("/Users/bautistapessagno/.cache/aleph/apps/meta.json").unwrap_or(false));

        //creo bien el index, pero encuentra cosas?
        let search = match app_search("Spotify.app") {
            Ok(top) => top,
            Err(e) => panic!("Error al buscar: {:?}", e),
        };
        //si llegamos hasta aca no hay errores, falta ver que busque bien
        assert!(!search.is_empty());

        assert!(search.contains(&(
            "Spotify.app".to_string(),
            "/Applications/Spotify.app".to_string()
        )));
    }
}

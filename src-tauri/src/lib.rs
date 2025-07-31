// Learn more about Tauri commands at https://tauri.app/develop/rust/

//manejo de paths y archivos
use jwalk::rayon::iter::{ParallelBridge, ParallelIterator};
use std::path::Path;
use std::{fs, path::PathBuf};
//tantivy
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{doc, Index, IndexWriter, Term};
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
// File system watching and time tracking
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

// Runtime estático reutilizable
static RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime")
});

// Global file status tracking
static FILE_STATUS: LazyLock<Arc<Mutex<HashMap<String, FileStatus>>>> = LazyLock::new(|| {
    Arc::new(Mutex::new(HashMap::new()))
});

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileStatus {
    pub last_modified: DateTime<Utc>,
    pub last_opened: Option<DateTime<Utc>>,
    pub access_count: u32,
    pub status: FileEventStatus,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum FileEventStatus {
    Created,
    Modified,
    Opened,
    Removed,
    Normal,
}

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

// Notify system implementation
pub async fn start_file_watcher(paths: Vec<PathBuf>) -> Result<(), String> {
    let (tx, mut rx) = mpsc::channel(1000);
    
    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.blocking_send(event);
            }
        },
        Config::default(),
    ).map_err(|e| e.to_string())?;

    // Watch all specified paths
    for path in paths {
        if path.exists() {
            watcher.watch(&path, RecursiveMode::Recursive)
                .map_err(|e| e.to_string())?;
        }
    }

    // Process events in background
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            handle_file_event(event).await;
        }
    });

    // Keep watcher alive by moving it to a background task
    tokio::spawn(async move {
        let _watcher = watcher;
        // Keep the watcher alive indefinitely
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
        }
    });

    Ok(())
}

async fn handle_file_event(event: Event) {
    let now = Utc::now();
    
    for path in event.paths {
        let path_str = path.display().to_string();
        
        // Skip hidden files and unwanted directories
        if should_skip_path(&path) {
            continue;
        }
        
        let status = match event.kind {
            EventKind::Create(_) => FileEventStatus::Created,
            EventKind::Modify(_) => FileEventStatus::Modified,
            EventKind::Remove(_) => FileEventStatus::Removed,
            EventKind::Access(_) => FileEventStatus::Opened,
            _ => FileEventStatus::Normal,
        };
        
        // Update file status (release lock before async operations)
        let should_add_to_index;
        let should_update_index;
        let should_remove_from_index;
        
        {
            if let Ok(mut file_status_map) = FILE_STATUS.lock() {
                let file_status = file_status_map.entry(path_str.clone()).or_insert(FileStatus {
                    last_modified: now,
                    last_opened: None,
                    access_count: 0,
                    status: FileEventStatus::Normal,
                });
                
                should_add_to_index = status == FileEventStatus::Created;
                should_update_index = status == FileEventStatus::Modified;
                should_remove_from_index = status == FileEventStatus::Removed;
                
                match status {
                    FileEventStatus::Created => {
                        file_status.status = FileEventStatus::Created;
                        file_status.last_modified = now;
                    },
                    FileEventStatus::Modified => {
                        file_status.status = FileEventStatus::Modified;
                        file_status.last_modified = now;
                    },
                    FileEventStatus::Opened => {
                        file_status.last_opened = Some(now);
                        file_status.access_count += 1;
                        if file_status.status == FileEventStatus::Normal {
                            file_status.status = FileEventStatus::Opened;
                        }
                    },
                    FileEventStatus::Removed => {
                        file_status.status = FileEventStatus::Removed;
                    },
                    _ => {}
                }
            } else {
                should_add_to_index = false;
                should_update_index = false;
                should_remove_from_index = false;
            }
        }
        
        // Perform index operations after releasing the lock
        if should_add_to_index {
            let _ = add_file_to_index(&path).await;
        } else if should_update_index {
            let _ = update_file_in_index(&path).await;
        } else if should_remove_from_index {
            let _ = remove_file_from_index(&path).await;
        }
    }
}

fn should_skip_path(path: &Path) -> bool {
    if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
        if file_name.starts_with('.') {
            return true;
        }
    }
    
    // Skip unwanted directories
    if path.is_dir() {
        if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
            let skip_dirs = [
                "node_modules", "target", "build", "dist", "tmp", "temp",
                ".git", ".svn", ".hg", "__pycache__", ".cache", ".vscode",
                ".idea", "coverage", ".nyc_output", "logs", "log"
            ];
            return skip_dirs.contains(&dir_name);
        }
    }
    
    // Skip unwanted file extensions
    if path.is_file() {
        let ext = path.extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        
        let skip_extensions = [
            "tmp", "temp", "bak", "swp", "swo", "log", "cache", 
            "lock", "pid", "DS_Store"
        ];
        
        if skip_extensions.contains(&ext.as_str()) {
            return true;
        }
    }
    
    false
}

// Index management functions
async fn add_file_to_index(file_path: &Path) -> Result<(), String> {
    if !file_path.is_file() {
        return Ok(());
    }
    
    // Determine which index to update based on path
    let home = dirs::home_dir().ok_or("Could not get home directory")?;
    let parent_dir = file_path.parent().ok_or("Could not get parent directory")?;
    
    // Find the appropriate index
    let mut index_folder = None;
    let mut current = parent_dir;
    
    while let Some(parent) = current.parent() {
        if parent == home {
            if let Some(folder_name) = current.file_name().and_then(|n| n.to_str()) {
                index_folder = Some(folder_name.to_string());
                break;
            }
        }
        current = parent;
    }
    
    if let Some(folder_name) = index_folder {
        let idx_path = home.join(".cache/aleph/index").join(&folder_name);
        
        if idx_path.exists() {
            if let Ok(index) = Index::open_in_dir(&idx_path) {
                let schema = index.schema();
                let path_f = schema.get_field("path").unwrap();
                let filename = schema.get_field("filename").unwrap();
                let ext_f = schema.get_field("extension").unwrap();
                
                if let Ok(mut writer) = index.writer_with_num_threads::<TantivyDocument>(1, 50_000_000) {
                    let path_str = file_path.display().to_string();
                    let name = file_path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_string();
                    let ext = file_path.extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    
                    let doc = doc!(
                        path_f => path_str,
                        filename => name.as_str(),
                        ext_f => ext.as_str(),
                    );
                    
                    writer.add_document(doc).map_err(|e| e.to_string())?;
                    writer.commit().map_err(|e| e.to_string())?;
                }
            }
        }
    }
    
    Ok(())
}

async fn update_file_in_index(file_path: &Path) -> Result<(), String> {
    // For updates, we remove and re-add the file
    remove_file_from_index(file_path).await?;
    add_file_to_index(file_path).await?;
    Ok(())
}

async fn remove_file_from_index(file_path: &Path) -> Result<(), String> {
    let home = dirs::home_dir().ok_or("Could not get home directory")?;
    let parent_dir = file_path.parent().ok_or("Could not get parent directory")?;
    
    // Find the appropriate index
    let mut index_folder = None;
    let mut current = parent_dir;
    
    while let Some(parent) = current.parent() {
        if parent == home {
            if let Some(folder_name) = current.file_name().and_then(|n| n.to_str()) {
                index_folder = Some(folder_name.to_string());
                break;
            }
        }
        current = parent;
    }
    
    if let Some(folder_name) = index_folder {
        let idx_path = home.join(".cache/aleph/index").join(&folder_name);
        
        if idx_path.exists() {
            if let Ok(index) = Index::open_in_dir(&idx_path) {
                let schema = index.schema();
                let path_f = schema.get_field("path").unwrap();
                
                if let Ok(mut writer) = index.writer_with_num_threads::<TantivyDocument>(1, 50_000_000) {
                    let path_str = file_path.display().to_string();
                    let term = Term::from_field_text(path_f, &path_str);
                    writer.delete_term(term);
                    writer.commit().map_err(|e| e.to_string())?;
                }
            }
        }
    }
    
    Ok(())
}

// Function to get file priority based on status
fn get_file_priority(path: &str) -> i32 {
    if let Ok(file_status_map) = FILE_STATUS.lock() {
        if let Some(status) = file_status_map.get(path) {
            let base_priority = match status.status {
                FileEventStatus::Created => 1000,
                FileEventStatus::Modified => 800,
                FileEventStatus::Opened => 600,
                FileEventStatus::Normal => 100,
                FileEventStatus::Removed => 0,
            };
            
            // Add access count bonus
            let access_bonus = (status.access_count as i32).min(100);
            
            // Add recency bonus (more recent = higher priority)
            let hours_since_modified = Utc::now()
                .signed_duration_since(status.last_modified)
                .num_hours();
            let recency_bonus = (24 - hours_since_modified.min(24)).max(0) as i32 * 10;
            
            return base_priority + access_bonus + recency_bonus;
        }
    }
    100 // Default priority
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
        .writer_with_num_threads::<TantivyDocument>(10, 200_000_000)
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
    
    // Función auxiliar para buscar en un índice específico con prioridad por status
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
                    // Get more results than needed for sorting
                    if let Ok(top_docs) = searcher.search(&parsed_query, &TopDocs::with_limit(limit * 2)) {
                        let mut scored_results = Vec::new();
                        
                        for (score, doc_address) in top_docs {
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
                                
                                // Calculate combined score (search relevance + file priority)
                                let file_priority = get_file_priority(&path) as f32;
                                let combined_score = score + (file_priority / 1000.0); // Normalize priority
                                
                                scored_results.push((combined_score, name, path));
                            }
                        }
                        
                        // Sort by combined score (higher is better)
                        scored_results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                        
                        // Take only the requested number of results
                        for (_, name, path) in scored_results.into_iter().take(limit) {
                            results.push((name, path));
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

    let mut schema_builder = Schema::builder();

    schema_builder.add_text_field("path", STORED | STRING);
    schema_builder.add_text_field("filename", TEXT | STORED);
    schema_builder.add_text_field("extension", STRING | STORED);

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
        .search(&query, &TopDocs::with_limit(20)) // Get more results for sorting
        .map_err(|e| e.to_string())?;

    let mut scored_results = Vec::new();

    for (score, doc_address) in top_docs {
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

        // Calculate combined score (search relevance + file priority)
        let file_priority = get_file_priority(&path) as f32;
        let combined_score = score + (file_priority / 1000.0); // Normalize priority
        
        scored_results.push((combined_score, name, path));
    }
    
    // Sort by combined score (higher is better)
    scored_results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    
    // Take only the top 10 results
    let top_docs_vec: Vec<(String, String)> = scored_results
        .into_iter()
        .take(10)
        .map(|(_, name, path)| (name, path))
        .collect();
        
    Ok(top_docs_vec)
}

#[tauri::command]
fn open_path(path: &str) -> Result<(), String> {
    // Update file status when opened
    let now = Utc::now();
    if let Ok(mut file_status_map) = FILE_STATUS.lock() {
        let file_status = file_status_map.entry(path.to_string()).or_insert(FileStatus {
            last_modified: now,
            last_opened: None,
            access_count: 0,
            status: FileEventStatus::Normal,
        });
        
        file_status.last_opened = Some(now);
        file_status.access_count += 1;
        if file_status.status == FileEventStatus::Normal {
            file_status.status = FileEventStatus::Opened;
        }
    }
    
    opener::open(path).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn start_notify_watcher() -> Result<(), String> {
    let home = dirs::home_dir().ok_or("Could not get home directory")?;
    
    // Watch common directories
    let watch_paths = vec![
        home.join("Desktop"),
        home.join("Documents"),
        home.join("Downloads"),
        home.join("Pictures"),
        home.join("Music"),
        home.join("Videos"),
    ];
    
    let existing_paths: Vec<PathBuf> = watch_paths
        .into_iter()
        .filter(|p| p.exists())
        .collect();
    
    start_file_watcher(existing_paths).await?;
    Ok(())
}

#[tauri::command]
fn get_file_status(path: &str) -> Result<Option<FileStatus>, String> {
    if let Ok(file_status_map) = FILE_STATUS.lock() {
        Ok(file_status_map.get(path).cloned())
    } else {
        Err("Could not access file status".to_string())
    }
}

#[tauri::command]
async fn manual_index_update(file_path: &str, action: &str) -> Result<(), String> {
    let path = Path::new(file_path);
    
    match action {
        "add" | "create" => {
            add_file_to_index(path).await?;
        },
        "update" | "modify" => {
            update_file_in_index(path).await?;
        },
        "remove" | "delete" => {
            remove_file_from_index(path).await?;
        },
        _ => {
            return Err(format!("Unknown action: {}", action));
        }
    }
    
    Ok(())
}

#[tauri::command]
fn clear_file_status() -> Result<(), String> {
    if let Ok(mut file_status_map) = FILE_STATUS.lock() {
        file_status_map.clear();
        Ok(())
    } else {
        Err("Could not clear file status".to_string())
    }
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
            start_notify_watcher,
            get_file_status,
            manual_index_update,
            clear_file_status
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

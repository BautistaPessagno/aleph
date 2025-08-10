use crate::icons;
use dirs;
use jwalk::rayon::iter::{ParallelBridge, ParallelIterator};
use jwalk::{Parallelism, WalkDir};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, QueryParser, RegexQuery};
use tantivy::schema::*;
use tantivy::Term;
use tantivy::TantivyError;
use tantivy::{doc, Index, IndexWriter};
use tokio;
// regex is referenced directly as `regex::...`

pub async fn create_index(path: &str) -> Result<(), String> {
    //El index se va a guardar en ~/.cache/aleph/index
    //Si no existe el path se crea
    let home = dirs::home_dir().unwrap();
    let idx_path = home.join(".cache/aleph/index").join(path);
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
    let root_dir = home.join(path);
    let root_dir = root_dir.as_path();

    WalkDir::new(root_dir)
        .skip_hidden(true)
        .follow_links(true)
        .parallelism(Parallelism::RayonNewPool(8))
        .into_iter()
        .par_bridge()
        .for_each(|res| {
            if let Ok(entry) = res {
                //filtro si no es un directorio
                if entry.file_type().is_file() {
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
pub async fn search_index(
    query: &str,
) -> Result<Vec<(String, String, f32, Option<String>)>, String> {
    //Deberia chequear si se creo el index
    let home = dirs::home_dir().unwrap();
    let idx_path = home.join(".cache/aleph/index");
    let idx_path = idx_path.as_path();

    let folders = [
        "Documents",
        "Downloads",
        "Pictures",
        "Music",
        "Movies",
        "Library",
        "Public",
    ];

    let search_in_index = |index: &Index,
                           limit: usize|
     -> Result<Vec<(String, String, f32, Option<String>)>, String> {
        let reader = index.reader().map_err(|e| e.to_string())?;
        // Ensure reader reflects the most recent commits (deletes/adds)
        reader.reload().map_err(|e| e.to_string())?;

        //let min_score = 0.1;
        let schema = index.schema();

        let path_f = schema.get_field("path").unwrap();
        let filename = schema.get_field("filename").unwrap();
        let ext_f = schema.get_field("extension").unwrap();

        let searcher = reader.searcher();

        let mut query_parser = QueryParser::for_index(&index, vec![path_f, filename, ext_f]);
        query_parser.set_field_fuzzy(filename, false, 1, true);

        let query_str = &query;
        let fuzzy_query = query_parser
            .parse_query(query)
            .map_err(|e| e.to_string())?;

        // Substring case-insensitive using RegexQuery on path
        let escaped = regex::escape(query);
        let ci_regex = format!("(?i).*{}.*", escaped);
        let substring_query = RegexQuery::from_pattern(&ci_regex, path_f)
            .map_err(|e| e.to_string())?;

        // Combine fuzzy filename match OR substring path match
        let combined = BooleanQuery::new(vec![
            (Occur::Should, Box::new(fuzzy_query)),
            (Occur::Should, Box::new(substring_query)),
        ]);

        let top_docs = searcher
            .search(&combined, &TopDocs::with_limit(limit))
            .map_err(|e| e.to_string())?;

        let mut first_vec: Vec<(String, String, f32, Option<String>)> =
            Vec::with_capacity(top_docs.len());

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

            // Drop stale entries that no longer exist on disk and eagerly clean the index
            if !std::path::Path::new(&path).exists() {
                let _ = delete_from_index(std::path::Path::new(&path));
                continue;
            }

            let extension = retrieved_doc
                .get_first(ext_f)
                .and_then(|v| v.as_str())
                .unwrap_or_default();

            let better_score = calculate_contextual_score(&name, &path, score, &query_str);

            // Get icon for the file
            let icon = if icons::is_executable(&path) {
                // If it's an app, extract app icon
                icons::extract_app_icon(&path)
            } else {
                // Otherwise get file type icon
                icons::get_file_icon(&path, extension)
            };

            first_vec.push((name, path, better_score, icon));
        }
        let top_docs_vec: Vec<(String, String, f32, Option<String>)> = first_vec
            .into_iter()
            //.filter(|(_, _, score, _)| *score > min_score)
            .collect();

        Ok(top_docs_vec)
    };

    if !idx_path.exists() {
        fs::create_dir_all(idx_path).map_err(|e| e.to_string())?;
    }

    let desktop_path = idx_path.join("Desktop");
    //  Abrir o crear el índice de forma segura
    let index = match Index::open_in_dir(&desktop_path) {
        Ok(idx) => idx, // lo encuentra, lo habre
        Err(_) => {
            // Índice no existe o hay otro error
            // Crear el índice y luego abrirlo
            create_index("Desktop").await.map_err(|e| e.to_string())?;
            let idx_path = home.join(".cache/aleph/index/Desktop");
            let idx_path = idx_path.as_path();
            Index::open_in_dir(idx_path).map_err(|e| e.to_string())?
        }
    };

    let destkop_dir = dirs::desktop_dir().unwrap();

    // Iniciar watchers una sola vez por ejecución
    static STARTED_WATCHERS: AtomicBool = AtomicBool::new(false);
    if !STARTED_WATCHERS.swap(true, Ordering::SeqCst) {
        // Desktop
        let desktop_watch = destkop_dir.clone();
        let _ = tokio::spawn(async move { let _ = async_watch(desktop_watch).await; });

        // Otros folders del Home
        let folders_dir = dirs::home_dir().unwrap();
        for folder in folders {
            let folder_dir = folders_dir.join(folder);
            let _ = tokio::spawn(async move { let _ = async_watch(folder_dir).await; });
        }
    }
    //hacemos un for para armar los otros índices si faltan
    for folder in folders {
        let folder_idx = idx_path.join(folder);
        match Index::open_in_dir(&folder_idx) {
            Ok(_) => {}
            Err(_) => {
                let _ = tokio::spawn(async move {
                    // Creamos el índice en background (el watcher ya está mirando el FS)
                    let _ = create_index(folder).await;
                });
            }
        };
    }

    //hacemos primero el de Desktop
    let mut results = search_in_index(&index, 5).unwrap();

    // Definir el score mínimo para considerar un resultado "excelente"
    // let excellent_score_threshold = 8.0; // Ajusta este valor según tus necesidades

    // Encontrar el mejor score actual
    // let best_score = results
    //     .iter()
    //     .map(|(_, _, score)| *score)
    //     .fold(0.0, f32::max);

    //si esta incompleto Y no tenemos un resultado excelente, completar con los otros folders
    // if results.len() < 15 && best_score < excellent_score_threshold {
    for folder in folders {
        let folder_idx = idx_path.join(folder);
        if folder_idx.exists() {
            match Index::open_in_dir(&folder_idx) {
                Ok(idx) => {
                    let mut new_result = search_in_index(&idx, 5)?;
                    results.append(&mut new_result);
                }
                Err(_) => {}
            }

            // Actualizar el mejor score después de agregar nuevos resultados
            // let current_best_score = results
            //     .iter()
            //     .map(|(_, _, score)| *score)
            //     .fold(0.0, f32::max);

            // if results.len() >= 15 || current_best_score >= excellent_score_threshold {
            // break;
            // }
        }

        // }
    }

    results.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
    results.truncate(15);

    Ok(results)
}

// Nueva función para scoring contextual
fn calculate_contextual_score(name: &str, path: &str, base_score: f32, query: &str) -> f32 {
    let mut score = base_score;

    let q = query.to_lowercase();
    let name_l = name.to_lowercase();
    let path_l = path.to_lowercase();

    // Prioridad: substring (regex-like) en el path por encima del fuzzy
    if path_l.contains(&q) {
        // Alto boost para coincidencia directa en el path completo
        score *= 3.0;
    } else if name_l.contains(&q) {
        // Boost moderado si coincide en el nombre
        score *= 1.5;
    }

    // A futuro, conseguir el el ultimo uso
    // Si falla (archivo borrado o sin metadata), no penalizamos
    score *= get_recency_boost(path).unwrap_or(1.0);

    // Penalizar archivos en directorios muy profundos
    let depth = path.matches('/').count();
    if depth > 6 {
        score *= 0.8;
    }

    // Boost para tipos de archivo comunes
    if name.ends_with(".txt") || name.ends_with(".pdf") || name.ends_with(".doc") {
        score *= 1.2;
    }

    score
}

const HALF_LIFE_DAYS: f32 = 30.0; // ajusta a tu gusto
const SECS_IN_DAY: f32 = 86_400.0;
const MIN: f32 = 0.2;
const MAX: f32 = 1.2;

fn get_recency_boost(path: &str) -> Result<f32, String> {
    let meta = fs::metadata(path).map_err(|e| e.to_string())?;
    let last_access_time = meta.atime();
    let half_life_secs = HALF_LIFE_DAYS * SECS_IN_DAY;
    let now_secs = chrono::Utc::now().timestamp();

    let age = (now_secs - last_access_time) as f32;
    let base: f32 = 0.5;

    let ans = base.powf(age / half_life_secs);

    Ok(MIN + (MAX - MIN) * ans)
    //ahora los criterios
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_test() {
        //Primero me fijo de que se cree el index
        let rt = tokio::runtime::Runtime::new().unwrap();

        //creo bien el index, pero encuentra cosas?
        let search = rt.block_on(async {
            match search_index("leetcode.c").await {
                Ok(top) => top,
                Err(e) => panic!("Error al buscar: {:?}", e),
            }
        });

        //si llegamos hasta aca no hay errores, falta ver que busque bien
        // assert!(!search.is_empty());

        assert!(search
            .iter()
            .any(|(a, b, _, _)| *a == "leetcode.c".to_string()
                && *b == "/Users/bautistapessagno/Desktop/leetcode.c".to_string()));
    }
}

use futures::{
    channel::mpsc::{channel, Receiver},
    SinkExt, StreamExt,
};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use notify::event::{EventKind, CreateKind};

// watcher para updates de cambios en los directorios
fn async_watcher() -> notify::Result<(RecommendedWatcher, Receiver<notify::Result<Event>>)> {
    let (mut tx, rx) = channel(100);

    // Automatically select the best implementation for your platform.
    // You can also access each implementation directly e.g. INotifyWatcher.
    let watcher = RecommendedWatcher::new(
        move |res| {
            futures::executor::block_on(async {
                tx.send(res).await.unwrap();
            })
        },
        Config::default(),
    )?;

    Ok((watcher, rx))
}

async fn async_watch<P: AsRef<Path>>(path: P) -> notify::Result<()> {
    let (mut watcher, mut rx) = async_watcher()?;

    if !path.as_ref().exists() {
        println!("❌ El path no existe: {:?}", path.as_ref());
        return Err(notify::Error::new(notify::ErrorKind::PathNotFound));
    }

    // Add a path to be watched. All files and directories at that path and
    // below will be monitored for changes.
    watcher.watch(path.as_ref(), RecursiveMode::Recursive)?;

    // Desduplicación global (path + tipo de evento) por una ventana corta
    static DEDUP_CACHE: OnceLock<Mutex<std::collections::HashMap<String, Instant>>> = OnceLock::new();
    const DEDUP_TTL: Duration = Duration::from_millis(500);

    while let Some(res) = rx.next().await {
        match res {
            Ok(event) => {
                if event.kind.is_create() || event.kind.is_remove() {
                    for changed_path in event.paths {
                        if changed_path.is_file() || event.kind.is_remove() {
                            // Clave de desduplicación
                            let key = format!(
                                "{}|{}",
                                changed_path.display(),
                                if event.kind.is_create() { "create" } else { "remove" }
                            );

                            let cache = DEDUP_CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
                            let mut map = cache.lock().unwrap();
                            let now = Instant::now();

                            // Limpieza de entradas viejas (lazy)
                            map.retain(|_, t| now.duration_since(*t) <= DEDUP_TTL);

                            // Si lo vimos hace muy poco, saltamos
                            let seen_recently = map.get(&key).map(|t| now.duration_since(*t) <= DEDUP_TTL).unwrap_or(false);
                            if seen_recently {
                                continue;
                            }
                            map.insert(key, now);

                            match &event.kind {
                                EventKind::Create(CreateKind::File) => {
                                    if let Err(e) = add_to_index(&changed_path) {
                                        println!("Error adding to index: {}", e);
                                    }
                                }
                                // Handle any kind of remove event (file or generic)
                                EventKind::Remove(_) => {
                                    if let Err(e) = delete_from_index(&changed_path) {
                                        println!("Error deleting from index: {}", e);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                // Pequeño debounce
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            }
            Err(e) => println!("watch error: {:?}", e),
        }
    }

    Ok(())
}

fn add_to_index(file_path: &Path) -> Result<(), String> {
    // Determinar a qué índice pertenece
    let (_folder_name, idx_dir) = match infer_folder_and_index_dir(file_path) {
        Some(v) => v,
        None => return Ok(()),
    };

    let (index, path_f, filename_f, ext_f) = open_or_create_index(&idx_dir)?;

    let mut writer: IndexWriter = index
        .writer_with_num_threads(2, 50_000_000)
        .map_err(|e| e.to_string())?;

    let absolute_path = if file_path.is_absolute() {
        file_path.to_path_buf()
    } else {
        std::env::current_dir().unwrap().join(file_path)
    };

    let path_str = absolute_path.display().to_string();
    let name = absolute_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let ext = absolute_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    let document = doc!(
        path_f => path_str,
        filename_f => name,
        ext_f => ext.as_str(),
    );
    writer.add_document(document).map_err(|e| e.to_string())?;
    writer.commit().map_err(|e| e.to_string())?;
    writer
        .wait_merging_threads()
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn delete_from_index(file_path: &Path) -> Result<(), String> {
    let (_, idx_dir) = match infer_folder_and_index_dir(file_path) {
        Some(v) => v,
        None => return Ok(()),
    };

    let (index, path_f, _filename_f, _ext_f) = open_or_create_index(&idx_dir)?;
    let mut writer: IndexWriter = index
        .writer_with_num_threads(2, 50_000_000)
        .map_err(|e| e.to_string())?;

    let absolute_path = if file_path.is_absolute() {
        file_path.to_path_buf()
    } else {
        std::env::current_dir().unwrap().join(file_path)
    };

    // Canonicalizar para evitar diferencias de representación del path
    let path_str = absolute_path.display().to_string();
    // Borrar el documento exacto
    let term = Term::from_field_text(path_f, &path_str);
    writer.delete_term(term);

    // Nota: si se elimina un directorio, este borrado exacto no elimina hijos.
    // Podemos ampliar luego a borrar por prefijo usando una query compatible con la versión de Tantivy.
    writer.commit().map_err(|e| e.to_string())?;
    writer
        .wait_merging_threads()
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn infer_folder_and_index_dir(file_path: &Path) -> Option<(String, PathBuf)> {
    let home = dirs::home_dir()?;

    // Construir pares (nombre_visible, ruta_absoluta)
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    if let Some(desktop_dir) = dirs::desktop_dir() {
        candidates.push(("Desktop".to_string(), desktop_dir));
    }
    for name in [
        "Documents",
        "Downloads",
        "Pictures",
        "Music",
        "Movies",
        "Library",
        "Public",
    ] {
        candidates.push((name.to_string(), home.join(name)));
    }

    // Buscar el primer candidato cuyo path sea prefijo del file_path
    for (display, abs_dir) in candidates {
        if file_path.starts_with(&abs_dir) {
            let idx_dir = home.join(".cache/aleph/index").join(&display);
            return Some((display, idx_dir));
        }
    }

    None
}

#[allow(dead_code)]
fn watched_folders() -> Vec<&'static str> {
    vec![
        "Documents",
        "Downloads",
        "Pictures",
        "Music",
        "Movies",
        "Library",
        "Public",
    ]
}

fn open_or_create_index(idx_dir: &Path) -> Result<(Index, Field, Field, Field), String> {
    if !idx_dir.exists() {
        fs::create_dir_all(idx_dir).map_err(|e| e.to_string())?;
    }

    // Definir el mismo schema que en create_index
    let mut schema_builder = Schema::builder();
    let _ = schema_builder.add_text_field("path", STORED | STRING);
    let _ = schema_builder.add_text_field("filename", TEXT | STORED);
    let _ = schema_builder.add_text_field("extension", STRING | STORED);
    let schema = schema_builder.build();

    let index = match Index::open_in_dir(idx_dir) {
        Ok(idx) => idx,
        Err(_) => Index::create_in_dir(idx_dir, schema).map_err(|e| e.to_string())?,
    };

    // Volver a obtener los fields desde el schema real del índice
    let s = index.schema();
    let path_f = s
        .get_field("path")
        .map_err(|_| "field path not found".to_string())?;
    let filename_f = s
        .get_field("filename")
        .map_err(|_| "field filename not found".to_string())?;
    let ext_f = s
        .get_field("extension")
        .map_err(|_| "field extension not found".to_string())?;

    Ok((index, path_f, filename_f, ext_f))
}

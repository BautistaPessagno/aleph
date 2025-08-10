use crate::icons;
use dirs;
use jwalk::rayon::iter::{ParallelBridge, ParallelIterator};
use jwalk::{Parallelism, WalkDir};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, QueryParser, RegexQuery};
use tantivy::schema::*;
use tantivy::{doc, Index, IndexWriter, Term};
use tantivy::TantivyError;
use tokio;
use futures::{
    channel::mpsc::{channel, Receiver},
    SinkExt, StreamExt,
};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use notify::event::{EventKind, CreateKind};

pub async fn create_app_launcher() -> Result<(), String> {
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
        Err(TantivyError::IndexAlreadyExists) => {
            Index::open_in_dir(idx_path).map_err(|e| e.to_string())?
        } // ya existía
        Err(e) => return Err(e.to_string()),
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
                    let _ = index_writer.add_document(doc);
                }
            }
        });
    index_writer.commit().map_err(|e| e.to_string())?;
    index_writer
        .wait_merging_threads()
        .map_err(|e| e.to_string())?;

    Ok(())
}

// watcher helpers (async) para `/Applications`
fn async_watcher() -> notify::Result<(RecommendedWatcher, Receiver<notify::Result<Event>>)> {
    let (mut tx, rx) = channel(100);
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

async fn async_watch_apps<P: AsRef<Path>>(path: P) -> notify::Result<()> {
    let (mut watcher, mut rx) = async_watcher()?;

    if !path.as_ref().exists() {
        println!("❌ El path no existe: {:?}", path.as_ref());
        return Err(notify::Error::new(notify::ErrorKind::PathNotFound));
    }

    watcher.watch(path.as_ref(), RecursiveMode::Recursive)?;

    static DEDUP_CACHE: OnceLock<Mutex<std::collections::HashMap<String, Instant>>> = OnceLock::new();
    const DEDUP_TTL: Duration = Duration::from_millis(700);

    while let Some(res) = rx.next().await {
        match res {
            Ok(event) => {
                if event.kind.is_create() || event.kind.is_remove() {
                    for changed_path in event.paths {
                        // Resolver el bundle `.app` asociado (si aplica)
                        if let Some(bundle_path) = resolve_app_bundle(&changed_path) {
                            let key = format!(
                                "{}|{}",
                                bundle_path.display(),
                                if event.kind.is_create() { "create" } else { "remove" }
                            );

                            let cache = DEDUP_CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
                            let mut map = cache.lock().unwrap();
                            let now = Instant::now();
                            map.retain(|_, t| now.duration_since(*t) <= DEDUP_TTL);
                            let seen_recently = map.get(&key).map(|t| now.duration_since(*t) <= DEDUP_TTL).unwrap_or(false);
                            if seen_recently { continue; }
                            map.insert(key, now);

                            match &event.kind {
                                EventKind::Create(CreateKind::File) | EventKind::Create(CreateKind::Folder) | EventKind::Create(CreateKind::Any) => {
                                    if let Err(e) = add_app_to_index(&bundle_path) {
                                        println!("Error adding app to index: {}", e);
                                    }
                                }
                                EventKind::Remove(_) => {
                                    if let Err(e) = delete_app_from_index(&bundle_path) {
                                        println!("Error deleting app from index: {}", e);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            }
            Err(e) => println!("watch error: {:?}", e),
        }
    }

    Ok(())
}

fn resolve_app_bundle(path: &Path) -> Option<PathBuf> {
    // si el path mismo es un bundle
    if path.extension().and_then(|s| s.to_str()).unwrap_or("") == "app" {
        return Some(path.to_path_buf());
    }
    // buscar en ancestros
    for ancestor in path.ancestors() {
        if ancestor.extension().and_then(|s| s.to_str()).unwrap_or("") == "app" {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn open_or_create_apps_index() -> Result<(Index, Field, Field, Field), String> {
    let home = dirs::home_dir().ok_or_else(|| "no home dir".to_string())?;
    let idx_dir = home.join(".cache/aleph/apps");
    if !idx_dir.exists() {
        fs::create_dir_all(&idx_dir).map_err(|e| e.to_string())?;
    }

    let mut schema_builder = Schema::builder();
    let _ = schema_builder.add_text_field("path", STORED | STRING);
    let _ = schema_builder.add_text_field("filename", TEXT | STORED);
    let _ = schema_builder.add_text_field("extension", STRING | STORED);
    let schema = schema_builder.build();

    let index = match Index::open_in_dir(&idx_dir) {
        Ok(idx) => idx,
        Err(_) => Index::create_in_dir(&idx_dir, schema).map_err(|e| e.to_string())?,
    };

    let s = index.schema();
    let path_f = s.get_field("path").map_err(|_| "field path not found".to_string())?;
    let filename_f = s.get_field("filename").map_err(|_| "field filename not found".to_string())?;
    let ext_f = s.get_field("extension").map_err(|_| "field extension not found".to_string())?;
    Ok((index, path_f, filename_f, ext_f))
}

fn add_app_to_index(bundle_path: &Path) -> Result<(), String> {
    if bundle_path.extension().and_then(|s| s.to_str()).unwrap_or("") != "app" {
        return Ok(());
    }
    let (index, path_f, filename_f, ext_f) = open_or_create_apps_index()?;
    let mut writer: IndexWriter = index
        .writer_with_num_threads(2, 50_000_000)
        .map_err(|e| e.to_string())?;

    let path_str = bundle_path.display().to_string();
    let name = bundle_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let ext = "app";

    let document = doc!(
        path_f => path_str,
        filename_f => name,
        ext_f => ext,
    );
    writer.add_document(document).map_err(|e| e.to_string())?;
    writer.commit().map_err(|e| e.to_string())?;
    writer.wait_merging_threads().map_err(|e| e.to_string())?;
    Ok(())
}

fn delete_app_from_index(bundle_path: &Path) -> Result<(), String> {
    if bundle_path.extension().and_then(|s| s.to_str()).unwrap_or("") != "app" {
        return Ok(());
    }
    let (index, path_f, _filename_f, _ext_f) = open_or_create_apps_index()?;
    let mut writer: IndexWriter = index
        .writer_with_num_threads(2, 50_000_000)
        .map_err(|e| e.to_string())?;

    let path_str = bundle_path.display().to_string();
    let term = Term::from_field_text(path_f, &path_str);
    writer.delete_term(term);
    writer.commit().map_err(|e| e.to_string())?;
    writer.wait_merging_threads().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn app_search(query: &str) -> Result<Vec<(String, String, Option<String>)>, String> {
    //Deberia chequear si se creo el index
    let home = dirs::home_dir().unwrap();
    let idx_path = home.join(".cache/aleph/apps");
    let idx_path = idx_path.as_path();

    if !idx_path.exists() {
        fs::create_dir_all(idx_path).map_err(|e| e.to_string())?;
    }

    //  Abrir o crear el índice de forma segura
    let index = match Index::open_in_dir(idx_path) {
        Ok(idx) => idx, // lo encuentra, lo abre
        Err(_) => {
            // Crear el índice y luego abrirlo
            create_app_launcher().await?;
            Index::open_in_dir(idx_path).map_err(|e| e.to_string())?
        }
    };

    let reader = index.reader().map_err(|e| e.to_string())?;
    reader.reload().map_err(|e| e.to_string())?;

    let schema = index.schema();

    let path_f = schema.get_field("path").unwrap();
    let filename = schema.get_field("filename").unwrap();
    let ext_f = schema.get_field("extension").unwrap();

    let searcher = reader.searcher();

    let mut query_parser = QueryParser::for_index(&index, vec![path_f, filename, ext_f]);
    query_parser.set_field_fuzzy(filename, false, 2, true);

    // Fuzzy por nombre + substring case-insensitive por path
    let fuzzy_query = query_parser.parse_query(query).map_err(|e| e.to_string())?;
    let escaped = regex::escape(query);
    let ci_regex = format!("(?i).*{}.*", escaped);
    let substring_query = RegexQuery::from_pattern(&ci_regex, path_f).map_err(|e| e.to_string())?;
    let combined = BooleanQuery::new(vec![
        (Occur::Should, Box::new(fuzzy_query)),
        (Occur::Should, Box::new(substring_query)),
    ]);

    let top_docs = searcher
        .search(&combined, &TopDocs::with_limit(15))
        .map_err(|e| e.to_string())?;

    let mut top_docs_vec: Vec<(String, String, Option<String>)> =
        Vec::with_capacity(top_docs.len());

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

        // Extract app icon
        let icon = icons::extract_app_icon(&path);

        top_docs_vec.push((name, path, icon));
    }
    // Start watcher una sola vez
    static STARTED_APPS_WATCHER: AtomicBool = AtomicBool::new(false);
    if !STARTED_APPS_WATCHER.swap(true, Ordering::SeqCst) {
        let apps_dir = PathBuf::from("/Applications");
        let _ = tokio::spawn(async move { let _ = async_watch_apps(apps_dir).await; });
    }

    Ok(top_docs_vec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apps() {
        //Primero me fijo de que se cree el index
        let rt = tokio::runtime::Runtime::new().unwrap();
        let ans = rt.block_on(async { create_app_launcher().await });
        match ans {
            Ok(_e) => println!("el index no paniqueo"),
            Err(e) => eprint!("hay algun error al crear el index {:?}", e),
        }
        assert!(fs::exists("/Users/bautistapessagno/.cache/aleph/apps/meta.json").unwrap_or(false));

        //creo bien el index, pero encuentra cosas?
        let search = rt.block_on(async {
            match app_search("Spotify.app").await {
                Ok(top) => top,
                Err(e) => panic!("Error al buscar: {:?}", e),
            }
        });
        //si llegamos hasta aca no hay errores, falta ver que busque bien
        assert!(!search.is_empty());

        assert!(search.iter().any(
            |(name, path, _icon)| name == "Spotify.app" && path == "/Applications/Spotify.app"
        ));
    }
}


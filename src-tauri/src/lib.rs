// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/

//manejo de paths y archivos
use jwalk::rayon::iter::{ParallelBridge, ParallelIterator};
use std::fs::create_dir_all;
use std::iter::Skip;
use std::path::Path;
use std::task;
use std::{fs, path::PathBuf};
//tantivy
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{self, *};
use tantivy::{doc, index, Index, IndexWriter};
use tantivy::{ReloadPolicy, TantivyError};
//WalkDir
use jwalk::{Parallelism, WalkDir};
//opener
use opener::{self, open, OpenError};
//dir
use dirs;
//tokio
use tokio;

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
async fn create_index() -> Result<(), String> {
    //El index se va a guardar en ~/.cache/aleph/index
    //Si no existe el path se crea
    let home = dirs::home_dir().unwrap();
    let idx_path = home.join(".cache/aleph/index");
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
    let root_dir = home.join("Desktop");
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
    index_writer.commit().map_err(|e| e.to_string());
    index_writer
        .wait_merging_threads()
        .map_err(|e| e.to_string());
    Ok(())
}

#[tauri::command]
fn search_index(query: &str) -> Result<Vec<(String, String)>, String> {
    //Deberia chequear si se creo el index
    let home = dirs::home_dir().unwrap();
    let idx_path = home.join(".cache/aleph/index");
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
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async { create_index().await.map_err(|e| e.to_string()) })?;
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
                    index_writer.add_document(doc);
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
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async { create_app_launcher().await.map_err(|e| e.to_string()) })?;
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
            create_index,
            search_index,
            open_path,
            create_app_launcher,
            app_search
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
        let rt = tokio::runtime::Runtime::new().unwrap();

        let ans = rt.block_on(async { create_index().await });

        match ans {
            Ok(_e) => println!("el index no paniqueo"),
            Err(e) => eprint!("hay algun error al crear el index {:?}", e),
        }
        assert!(
            fs::exists("/Users/bautistapessagno/.cache/aleph/index/meta.json").unwrap_or(false)
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
        let rt = tokio::runtime::Runtime::new().unwrap();
        let ans = rt.block_on(async { create_app_launcher().await });
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

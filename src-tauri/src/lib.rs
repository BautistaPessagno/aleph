// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/

//manejo de paths y archivos
use jwalk::rayon::iter::{ParallelBridge, ParallelIterator};
use std::path::Path;
use std::{fs, sync::Arc};
//tantivy
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::TantivyError;
use tantivy::{doc, Index, IndexWriter};
//WalkDir
use jwalk::{Parallelism, WalkDir};

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

fn create_index() -> tantivy::Result<()> {
    //El index se va a guardar en ~/.cache/aleph/index
    //Si no existe el path se crea
    let idx_path = Path::new("~/.cache/aleph/index");

    if !idx_path.exists() {
        fs::create_dir_all("~/.cache/aleph/index");
    }

    let mut schema_builder = Schema::builder();

    schema_builder.add_text_field("path", STORED | STRING);
    schema_builder.add_text_field("filename", TEXT | STORED);
    schema_builder.add_text_field("extension", STRING | STORED);

    let schema = schema_builder.build();

    let index = Index::create_in_dir(&idx_path, schema.clone())?;

    let mut index_writer: IndexWriter = index.writer_with_num_threads(4, 50_000_000)?;
    //B: let writer = Arc::new(index.writer(50_000_000)?);

    let path_f = schema.get_field("path").unwrap();
    let filename = schema.get_field("filename").unwrap();
    let ext_f = schema.get_field("extension").unwrap();

    //Vamos a indexar todo
    let root_dir = Path::new("~/Desktop/");

    WalkDir::new(root_dir)
        .skip_hidden(true)
        .follow_links(true)
        .parallelism(Parallelism::RayonNewPool(8))
        .process_read_dir(|_depth, _path, _parent, children| {
            children.retain(|entry_result| {
                entry_result
                    .as_ref()
                    .map(|e| {
                        let name = e.file_name().to_string_lossy();
                        e.file_type().is_file() && !name.starts_with(".")
                    })
                    .unwrap_or(false)
            });
        })
        .into_iter()
        .par_bridge()
        .for_each(|res| {
            if let Ok(entry) = res {
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
        });
    index_writer.commit()?;

    Ok(())
}

fn search_index(path: &Path, query: &str) -> Result<Vec<(String, String)>, TantivyError> {
    let index = Index::open_in_dir(path)?;

    let reader = index.reader()?;

    let schema = index.schema();

    let path_f = schema.get_field("path").unwrap();
    let filename = schema.get_field("filename").unwrap();
    let ext_f = schema.get_field("extension").unwrap();

    let searcher = reader.searcher();

    let mut query_parser = QueryParser::for_index(&index, vec![path_f, filename, ext_f]);
    query_parser.set_field_fuzzy(filename, false, 2, true);

    let query = query_parser.parse_query(query)?;

    let top_docs = searcher.search(&query, &TopDocs::with_limit(10))?;

    let mut top_docs_vec: Vec<(String, String)> = Vec::with_capacity(top_docs.len());

    for (_score, doc_address) in top_docs {
        let retrieved_doc: TantivyDocument = searcher.doc(doc_address)?;
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
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![greet])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

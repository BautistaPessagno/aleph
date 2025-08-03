use jwalk::rayon::iter::{ParallelBridge, ParallelIterator};
use std::fs;
use std::path::Path;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{doc, Index, IndexWriter};
use jwalk::{Parallelism, WalkDir};
use dirs;
use tokio;
use crate::icons;

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

#[tauri::command]
pub fn app_search(query: &str) -> Result<Vec<(String, String, Option<String>)>, String> {
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

    let mut top_docs_vec: Vec<(String, String, Option<String>)> = Vec::with_capacity(top_docs.len());

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
        let search = match app_search("Spotify.app") {
            Ok(top) => top,
            Err(e) => panic!("Error al buscar: {:?}", e),
        };
        //si llegamos hasta aca no hay errores, falta ver que busque bien
        assert!(!search.is_empty());

        assert!(search.iter().any(|(name, path, _icon)| 
            name == "Spotify.app" && path == "/Applications/Spotify.app"
        ));
    }
}
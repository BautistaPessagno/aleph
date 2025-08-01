use dirs;
use jwalk::rayon::iter::{ParallelBridge, ParallelIterator};
use jwalk::{Parallelism, WalkDir};
use notify::Result as notify_Result;
use notify::{Event, RecursiveMode, Watcher};

use std::fs;
use std::{path::Path, sync::mpsc};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::TantivyError;
use tantivy::{doc, Index, IndexWriter};
use tokio;

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
pub async fn search_index(query: &str) -> Result<Vec<(String, String, f32)>, String> {
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

    let search_in_index =
        |index: &Index, limit: usize| -> Result<Vec<(String, String, f32)>, String> {
            let reader = index.reader().map_err(|e| e.to_string())?;

            let min_score = 0.5;
            let schema = index.schema();

            let path_f = schema.get_field("path").unwrap();
            let filename = schema.get_field("filename").unwrap();
            let ext_f = schema.get_field("extension").unwrap();

            let searcher = reader.searcher();

            let mut query_parser = QueryParser::for_index(&index, vec![path_f, filename, ext_f]);
            query_parser.set_field_fuzzy(filename, false, 1, true);

            let query_str = &query;
            let query = query_parser.parse_query(query).map_err(|e| e.to_string())?;

            let top_docs = searcher
                .search(&query, &TopDocs::with_limit(limit))
                .map_err(|e| e.to_string())?;

            let mut first_vec: Vec<(String, String, f32)> = Vec::with_capacity(top_docs.len());

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

                let better_score = calculate_contextual_score(&name, &path, score, &query_str);

                first_vec.push((name, path, better_score));
            }
            let top_docs_vec: Vec<(String, String, f32)> = first_vec
                .into_iter()
                .filter(|(_, _, score)| *score > min_score)
                .collect();

            Ok(top_docs_vec)
        };

    if !idx_path.exists() {
        fs::create_dir_all(idx_path).map_err(|e| e.to_string())?;
    }

    let desktop_path = idx_path.join("Desktop");
    //  Abrir o crear el índice de forma segura
    let index = match Index::open_in_dir(desktop_path) {
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

    //hacemos un for para armar los otros
    for folder in folders {
        let folder_idx = idx_path.join(folder);
        match Index::open_in_dir(&folder_idx) {
            Ok(_) => {}
            Err(_) => {
                let _ = tokio::spawn(async move { create_index(folder).await });
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

    // Boost para coincidencia exacta en el nombre
    if name.to_lowercase().contains(&query.to_lowercase()) {
        score *= 1.5;
    }

    // A futuro, conseguir el el ultimo uso, estoy medio verde para esas
    // score *= get_recency_boost(path);

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

        assert!(search.iter().any(|(a, b, _)| *a == "leetcode.c".to_string()
            && *b == "/Users/bautistapessagno/Desktop/leetcode.c".to_string()));
    }
}

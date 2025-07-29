import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openPath } from "@tauri-apps/plugin-opener";
import "./App.css";

interface SearchResult {
  name: string;
  path: string;
  isApp?: boolean;
}

function App() {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SearchResult[]>([]);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [isLoading, setIsLoading] = useState(false);
  const [indexCreated, setIndexCreated] = useState(false);

  // Crear índice al iniciar la aplicación
  useEffect(() => {
    const createIndex = async () => {
      try {
        setIsLoading(true);
        await invoke("create_index");
        setIndexCreated(true);
      } catch (error) {
        console.error("Error creating index:", error);
      } finally {
        setIsLoading(false);
      }
    };
    
    createIndex();
  }, []);

  // Función para detectar si un archivo es una aplicación
  const isApplication = (path: string, name: string): boolean => {
    const lowerPath = path.toLowerCase();
    const lowerName = name.toLowerCase();
    
    // macOS applications
    if (lowerName.endsWith('.app')) return true;
    
    // Common executable extensions
    const executableExtensions = ['.exe', '.app', '.dmg', '.pkg'];
    return executableExtensions.some(ext => lowerName.endsWith(ext));
  };

  // Función de búsqueda con debounce
  const searchFiles = useCallback(async (searchQuery: string) => {
    if (!searchQuery.trim() || !indexCreated) {
      setResults([]);
      return;
    }

    try {
      setIsLoading(true);
      const searchResults = await invoke<[string, string][]>("search_index", { 
        query: searchQuery 
      });
      
      const formattedResults: SearchResult[] = searchResults.map(([name, path]) => ({
        name,
        path,
        isApp: isApplication(path, name)
      }));

      // Priorizar aplicaciones en los resultados
      const sortedResults = formattedResults.sort((a, b) => {
        if (a.isApp && !b.isApp) return -1;
        if (!a.isApp && b.isApp) return 1;
        return 0;
      });

      setResults(sortedResults);
      setSelectedIndex(0);
    } catch (error) {
      console.error("Error searching:", error);
      setResults([]);
    } finally {
      setIsLoading(false);
    }
  }, [indexCreated]);

  // Debounce para la búsqueda
  useEffect(() => {
    const timeoutId = setTimeout(() => {
      searchFiles(query);
    }, 300);

    return () => clearTimeout(timeoutId);
  }, [query, searchFiles]);

  // Manejar navegación con teclado
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (results.length === 0) return;

      switch (e.key) {
        case 'ArrowDown':
          e.preventDefault();
          setSelectedIndex(prev => 
            prev < results.length - 1 ? prev + 1 : 0
          );
          break;
        case 'ArrowUp':
          e.preventDefault();
          setSelectedIndex(prev => 
            prev > 0 ? prev - 1 : results.length - 1
          );
          break;
        case 'Enter':
          e.preventDefault();
          if (results[selectedIndex]) {
            openItem(results[selectedIndex]);
          }
          break;
        case 'Escape':
          setQuery("");
          setResults([]);
          break;
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [results, selectedIndex]);

  // Función para abrir archivo/aplicación
  const openItem = async (item: SearchResult) => {
    try {
      await openPath(item.path);
      // Limpiar búsqueda después de abrir
      setQuery("");
      setResults([]);
    } catch (error) {
      console.error("Error opening item:", error);
    }
  };

  // Obtener ícono según el tipo de archivo
  const getItemIcon = (item: SearchResult) => {
    if (item.isApp) return "🚀";
    
    const extension = item.name.split('.').pop()?.toLowerCase();
    switch (extension) {
      case 'txt': case 'md': case 'rtf': return "📄";
      case 'pdf': return "📕";
      case 'jpg': case 'jpeg': case 'png': case 'gif': case 'bmp': return "🖼️";
      case 'mp4': case 'mov': case 'avi': case 'mkv': return "🎬";
      case 'mp3': case 'wav': case 'flac': case 'aac': return "🎵";
      case 'zip': case 'rar': case '7z': case 'tar': return "📦";
      case 'js': case 'ts': case 'py': case 'java': case 'cpp': case 'c': return "💻";
      default: return "📁";
    }
  };

  if (!indexCreated && isLoading) {
    return (
      <div className="app">
        <div className="loading">
          <div className="spinner"></div>
          <p>Indexing files...</p>
        </div>
      </div>
    );
  }

  return (
    <div className="app">
      <div className="search-container">
        <div className="search-box">
          <span className="search-icon">🔍</span>
          <input
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search files and applications..."
            className="search-input"
            autoFocus
          />
        </div>
        
        {isLoading && query && (
          <div className="loading-indicator">
            <div className="spinner small"></div>
          </div>
        )}

        {results.length > 0 && (
          <div className="results-container">
            {results.map((item, index) => (
              <div
                key={`${item.path}-${index}`}
                className={`result-item ${index === selectedIndex ? 'selected' : ''}`}
                onClick={() => openItem(item)}
                onMouseEnter={() => setSelectedIndex(index)}
              >
                <span className="item-icon">{getItemIcon(item)}</span>
                <div className="item-info">
                  <div className="item-name">{item.name}</div>
                  <div className="item-path">{item.path}</div>
                </div>
                {item.isApp && <span className="app-badge">APP</span>}
              </div>
            ))}
          </div>
        )}

        {query && !isLoading && results.length === 0 && (
          <div className="no-results">
            <p>No results found for "{query}"</p>
          </div>
        )}
      </div>

      <div className="help-text">
        <p>Type to search • ↑↓ to navigate • Enter to open • Esc to clear</p>
      </div>
    </div>
  );
}

export default App;

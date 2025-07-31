import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

interface SearchResult {
  name: string;
  path: string;
  isApp?: boolean;
  icon?: string; // base64 encoded icon
}

type SearchMode = 'apps' | 'files';

function App() {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SearchResult[]>([]);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [isLoading, setIsLoading] = useState(false);
  const [searchMode, setSearchMode] = useState<SearchMode>('apps');
  const [indexingStatus, setIndexingStatus] = useState({
    files: 'not_created', // 'not_created' | 'creating' | 'ready' | 'error'
    apps: 'not_created'
  });

  // Detectar si el índice existe usando una búsqueda de prueba
  const checkIndexStatus = useCallback(async (indexType: 'apps' | 'files') => {
    try {
      // Intentar una búsqueda vacía para ver si el índice existe
      if (indexType === 'apps') {
        await invoke<[string, string][]>("app_search", { query: "__test_empty__" });
      } else {
        await invoke<[string, string][]>("search_index", { query: "__test_empty__" });
      }
      
      // Si llegamos aquí, el índice existe y está listo
      setIndexingStatus(prev => ({ ...prev, [indexType]: 'ready' }));
      return 'ready';
      
    } catch (error) {
      // Si falla, puede ser que el índice no existe o se está creando
      setIndexingStatus(prev => ({ ...prev, [indexType]: 'not_created' }));
      return 'not_created';
    }
  }, []);

  // Función para detectar si un archivo es una aplicación
  const isApplication = (_path: string, name: string): boolean => {
    const lowerName = name.toLowerCase();
    
    // macOS applications
    if (lowerName.endsWith('.app')) return true;
    
    // Common executable extensions
    const executableExtensions = ['.exe', '.app', '.dmg', '.pkg'];
    return executableExtensions.some(ext => lowerName.endsWith(ext));
  };

  // Función de búsqueda que deja que Rust maneje la creación de índices automáticamente
  const searchFiles = useCallback(async (searchQuery: string) => {
    if (!searchQuery.trim()) {
      setResults([]);
      return;
    }

    try {
      setIsLoading(true);
      
      const indexType = searchMode === 'apps' ? 'apps' : 'files';
      
      // Si es la primera búsqueda en este índice, marcarlo como "creando"
      if (indexingStatus[indexType] === 'not_created') {
        setIndexingStatus(prev => ({ ...prev, [indexType]: 'creating' }));
      }

      let searchResults: [string, string][] = [];

      if (searchMode === 'apps') {
        searchResults = await invoke<[string, string][]>("app_search", { 
          query: searchQuery 
        });
      } else {
        searchResults = await invoke<[string, string][]>("search_index", { 
          query: searchQuery 
        });
      }
      
      // Si llegamos aquí, el índice está listo
      setIndexingStatus(prev => ({ ...prev, [indexType]: 'ready' }));
      
      const formattedResults: SearchResult[] = searchResults.map(([name, path]) => ({
        name,
        path,
        isApp: searchMode === 'apps' || isApplication(path, name)
      }));

      // Para el modo apps, todos los resultados son apps
      // Para files, priorizar aplicaciones si las hay
      const sortedResults = searchMode === 'apps' 
        ? formattedResults 
        : formattedResults.sort((a, b) => {
            if (a.isApp && !b.isApp) return -1;
            if (!a.isApp && b.isApp) return 1;
            return 0;
          });

      // Load icons for apps
      const resultsWithIcons = await Promise.all(
        sortedResults.map(async (result) => {
          if (result.isApp) {
            const icon = await loadAppIcon(result.path);
            return { ...result, icon };
          }
          return result;
        })
      );

      setResults(resultsWithIcons);
      setSelectedIndex(0);
    } catch (error) {
      console.error("Error searching:", error);
      const indexType = searchMode === 'apps' ? 'apps' : 'files';
      setIndexingStatus(prev => ({ ...prev, [indexType]: 'error' }));
      setResults([]);
    } finally {
      setIsLoading(false);
    }
  }, [searchMode, indexingStatus]);

  // Debounce para la búsqueda
  useEffect(() => {
    const timeoutId = setTimeout(() => {
      searchFiles(query);
    }, 300);

    return () => clearTimeout(timeoutId);
  }, [query, searchFiles]);

  // Verificar al inicio qué índices ya están creados
  useEffect(() => {
    const checkInitialIndexStatus = async () => {
      await Promise.all([
        checkIndexStatus('apps'),
        checkIndexStatus('files')
      ]);
    };
    
    checkInitialIndexStatus();
  }, [checkIndexStatus]);

  // Limpiar resultados cuando cambia el modo de búsqueda
  useEffect(() => {
    setQuery("");
    setResults([]);
    setSelectedIndex(0);
  }, [searchMode]);

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

  // Función para cargar iconos de aplicaciones
  const loadAppIcon = async (appPath: string): Promise<string | null> => {
    try {
      const iconBase64 = await invoke<string>("get_app_icon", { appPath });
      return iconBase64;
    } catch (error) {
      console.error("Error loading app icon:", error);
      return null;
    }
  };

  // Función para abrir archivo/aplicación
  const openItem = async (item: SearchResult) => {
    try {
      await invoke("open_path", { path: item.path });
      // Limpiar búsqueda después de abrir
      setQuery("");
      setResults([]);
    } catch (error) {
      console.error("Error opening item:", error);
    }
  };

  // Formatear nombre para visualización (remover .app de aplicaciones)
  const getDisplayName = (item: SearchResult) => {
    if (item.isApp && item.name.toLowerCase().endsWith('.app')) {
      return item.name.slice(0, -4); // Remover ".app"
    }
    return item.name;
  };

  // Obtener ícono según el tipo de archivo
  const getItemIcon = (item: SearchResult) => {
    // For apps, use the actual icon if available, otherwise fallback to emoji
    if (item.isApp && item.icon) {
      return <img src={`data:image/png;base64,${item.icon}`} alt="" className="app-icon" />;
    }
    
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

  return (
    <div className="app">
      <div className="search-container">
        {/* Mode Selector */}
        <div className="mode-selector">
          <button
            className={`mode-button ${searchMode === 'apps' ? 'active' : ''}`}
            onClick={() => setSearchMode('apps')}
          >
            🚀 Apps
            {indexingStatus.apps === 'creating' && <span className="indexing-indicator">⚡</span>}
          </button>
          <button
            className={`mode-button ${searchMode === 'files' ? 'active' : ''}`}
            onClick={() => setSearchMode('files')}
          >
            📁 Files
            {indexingStatus.files === 'creating' && <span className="indexing-indicator">⚡</span>}
          </button>
        </div>

        <div className="search-box">
          <span className="search-icon">🔍</span>
          <input
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={
              searchMode === 'apps' 
                ? (indexingStatus.apps === 'creating' ? "Creating app index..." : "Search applications...")
                : (indexingStatus.files === 'creating' ? "Creating file index..." : "Search files...")
            }
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
                  <div className="item-name">{getDisplayName(item)}</div>
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
            {(
              (searchMode === 'apps' && indexingStatus.apps === 'not_created') ||
              (searchMode === 'files' && indexingStatus.files === 'not_created')
            ) && (
              <div className="indexing-hints">
                <p className="lazy-hint">
                  {searchMode === 'files' 
                    ? "File index will be created automatically on first search"
                    : "App index will be created automatically on first search"
                  }
                </p>
              </div>
            )}
          </div>
        )}
      </div>

      <div className="help-text">
        <p>Type to search • ↑↓ to navigate • Enter to open • Esc to clear</p>
        <div className="indexing-status">
          <span className={`status-badge ${
            indexingStatus.apps === 'ready' ? 'ready' : 
            indexingStatus.apps === 'creating' ? 'indexing' : 
            'not-created'
          }`}>
            🚀 Apps: {
              indexingStatus.apps === 'ready' ? 'Ready' :
              indexingStatus.apps === 'creating' ? 'Creating...' :
              indexingStatus.apps === 'error' ? 'Error' :
              'Not Created'
            }
          </span>
          <span className={`status-badge ${
            indexingStatus.files === 'ready' ? 'ready' : 
            indexingStatus.files === 'creating' ? 'indexing' : 
            'not-created'
          }`}>
            📁 Files: {
              indexingStatus.files === 'ready' ? 'Ready' :
              indexingStatus.files === 'creating' ? 'Creating...' :
              indexingStatus.files === 'error' ? 'Error' :
              'Not Created'
            }
          </span>
        </div>
      </div>
    </div>
  );
}

export default App;

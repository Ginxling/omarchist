use super::color_extraction::ColorExtractor;
use super::get_sys_themes::SysTheme;
use crate::types::ThemeColors;
use dirs;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

/// Lightweight theme metadata for faster initial responses
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ThemeMetadata {
    pub dir: String,
    pub title: String,
    pub is_system: bool,
    pub is_custom: bool,
    pub has_colors: bool,
    pub has_image: bool,
}

/// Color extraction cache to avoid recomputation
#[derive(Debug, Clone)]
pub struct ColorCache {
    cache: Arc<RwLock<HashMap<String, Option<ThemeColors>>>>,
}

impl Default for ColorCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ColorCache {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get cached colors for a theme directory
    pub async fn get(&self, theme_dir: &str) -> Option<Option<ThemeColors>> {
        let cache = self.cache.read().await;
        cache.get(theme_dir).cloned()
    }

    /// Cache colors for a theme directory
    pub async fn set(&self, theme_dir: String, colors: Option<ThemeColors>) {
        let mut cache = self.cache.write().await;
        cache.insert(theme_dir, colors);
    }

    /// Clear the cache
    pub async fn clear(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }

    /// Get cache size
    pub async fn size(&self) -> usize {
        let cache = self.cache.read().await;
        cache.len()
    }
}

/// Optimized theme loader with parallel processing and caching
pub struct OptimizedThemeLoader {
    color_cache: ColorCache,
}

impl OptimizedThemeLoader {
    pub fn new() -> Self {
        Self {
            color_cache: ColorCache::new(),
        }
    }

    /// Optimized helper function to convert directory name to title
    fn dir_name_to_title(dir_name: &str) -> String {
        let mut title = String::with_capacity(dir_name.len() + 10);
        let mut capitalize_next = true;

        for ch in dir_name.chars() {
            match ch {
                '-' | '_' => {
                    title.push(' ');
                    capitalize_next = true;
                },
                c if capitalize_next => {
                    title.extend(c.to_uppercase());
                    capitalize_next = false;
                },
                c => {
                    title.push(c);
                },
            }
        }
        title
    }

    /// Load themes with parallel processing for better performance
    pub async fn load_themes_parallel(&self) -> Result<Vec<SysTheme>, String> {
        let home_dir =
            dirs::home_dir().ok_or_else(|| "Failed to get home directory".to_string())?;
        let themes_dir = home_dir.join(".config/omarchy/themes");

        if !themes_dir.exists() {
            return Err(format!("Themes directory does not exist: {themes_dir:?}"));
        }

        // Collect all theme directory paths
        let theme_paths = self.collect_theme_paths(&themes_dir)?;

        if theme_paths.is_empty() {
            return Ok(Vec::new());
        }

        log::info!(
            "Loading {} themes with parallel processing",
            theme_paths.len()
        );

        // Process themes in parallel using tokio::spawn
        let mut handles: Vec<JoinHandle<Result<SysTheme, String>>> = Vec::new();

        for path in theme_paths {
            let color_cache = self.color_cache.clone();
            let handle = tokio::spawn(async move {
                Self::generate_theme_from_directory_async(&path, color_cache).await
            });
            handles.push(handle);
        }

        // Collect results from all parallel tasks
        let mut themes = Vec::new();
        let mut errors = Vec::new();

        for handle in handles {
            match handle.await {
                Ok(Ok(theme)) => themes.push(theme),
                Ok(Err(e)) => errors.push(e),
                Err(e) => errors.push(format!("Task join error: {e}")),
            }
        }

        // Log any errors but continue with successful themes
        if !errors.is_empty() {
            log::warn!(
                "Encountered {} errors during parallel theme loading: {:?}",
                errors.len(),
                errors
            );
        }

        log::info!("Successfully loaded {} themes in parallel", themes.len());
        Ok(themes)
    }

    /// Load only theme metadata for faster initial responses
    pub async fn load_theme_metadata_only(&self) -> Result<Vec<ThemeMetadata>, String> {
        let home_dir =
            dirs::home_dir().ok_or_else(|| "Failed to get home directory".to_string())?;
        let themes_dir = home_dir.join(".config/omarchy/themes");

        if !themes_dir.exists() {
            return Err(format!("Themes directory does not exist: {themes_dir:?}"));
        }

        let theme_paths = self.collect_theme_paths(&themes_dir)?;

        if theme_paths.is_empty() {
            return Ok(Vec::new());
        }

        log::info!("Loading metadata for {} themes", theme_paths.len());

        // Process metadata in parallel
        let mut handles: Vec<JoinHandle<Result<ThemeMetadata, String>>> = Vec::new();

        for path in theme_paths {
            let handle = tokio::spawn(async move { Self::generate_theme_metadata(&path).await });
            handles.push(handle);
        }

        // Collect metadata results
        let mut metadata = Vec::new();
        let mut errors = Vec::new();

        for handle in handles {
            match handle.await {
                Ok(Ok(meta)) => metadata.push(meta),
                Ok(Err(e)) => errors.push(e),
                Err(e) => errors.push(format!("Metadata task join error: {e}")),
            }
        }

        if !errors.is_empty() {
            log::warn!(
                "Encountered {} errors during metadata loading: {:?}",
                errors.len(),
                errors
            );
        }

        log::info!("Successfully loaded metadata for {} themes", metadata.len());
        Ok(metadata)
    }

    /// Collect all theme directory paths
    fn collect_theme_paths(&self, themes_dir: &Path) -> Result<Vec<PathBuf>, String> {
        let entries = fs::read_dir(themes_dir)
            .map_err(|e| format!("Failed to read themes directory: {e}"))?;

        let mut theme_paths = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| format!("Failed to read directory entry: {e}"))?;
            let path = entry.path();

            if path.is_dir() {
                theme_paths.push(path);
            }
        }

        Ok(theme_paths)
    }

    /// Generate theme metadata only (lightweight operation)
    async fn generate_theme_metadata(theme_dir: &Path) -> Result<ThemeMetadata, String> {
        let dir_name = theme_dir
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "Invalid directory name".to_string())?;

        // Convert directory name to a nice title (optimized)
        let title = Self::dir_name_to_title(dir_name);

        let is_custom = theme_dir.join("custom_theme.json").is_file();

        // Check if the theme directory is a symlink (system theme)
        let is_system = if is_custom {
            false
        } else {
            fs::symlink_metadata(theme_dir)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
        };

        // Check if theme has color configuration files
        let has_colors = theme_dir.join("custom_theme.json").exists()
            || theme_dir.join("alacritty.toml").exists();

        // Check if theme has image files
        let has_image = Self::has_image_files(theme_dir);

        Ok(ThemeMetadata {
            dir: dir_name.to_string(),
            title,
            is_system,
            is_custom,
            has_colors,
            has_image,
        })
    }

    /// Check if directory contains image files
    fn has_image_files(theme_dir: &Path) -> bool {
        if let Ok(entries) = fs::read_dir(theme_dir) {
            for entry in entries.flatten() {
                let file_path = entry.path();
                if file_path.is_file() {
                    if let Some(extension) = file_path.extension().and_then(|ext| ext.to_str()) {
                        let ext_lower = extension.to_lowercase();
                        if matches!(
                            ext_lower.as_str(),
                            "png" | "jpg" | "jpeg" | "webp" | "gif" | "svg"
                        ) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// Generate full theme from directory with async color extraction and caching
    async fn generate_theme_from_directory_async(
        theme_dir: &Path,
        color_cache: ColorCache,
    ) -> Result<SysTheme, String> {
        let dir_name = theme_dir
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "Invalid directory name".to_string())?;

        // Convert directory name to a nice title (optimized)
        let title = Self::dir_name_to_title(dir_name);

        let is_custom = theme_dir.join("custom_theme.json").is_file();

        // Check if the theme directory is a symlink (system theme)
        let is_system = if is_custom {
            false
        } else {
            fs::symlink_metadata(theme_dir)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
        };

        // Extract colors with caching
        let colors = Self::extract_theme_colors_cached(theme_dir, is_custom, &color_cache).await;

        // Load image asynchronously
        let image_path = Self::load_theme_image_async(theme_dir).await;

        Ok(SysTheme {
            dir: dir_name.to_string(),
            title,
            description: format!("Auto-generated theme from {dir_name}"),
            image: image_path,
            is_system,
            is_custom,
            colors,
        })
    }

    /// Extract theme colors with caching to avoid recomputation
    async fn extract_theme_colors_cached(
        theme_dir: &Path,
        is_custom: bool,
        color_cache: &ColorCache,
    ) -> Option<ThemeColors> {
        let dir_name = theme_dir.file_name()?.to_str()?.to_string();

        // Check cache first
        if let Some(cached_colors) = color_cache.get(&dir_name).await {
            log::debug!("Using cached colors for theme: {dir_name}");
            return cached_colors;
        }

        // Extract colors if not cached
        let colors = Self::extract_theme_colors_direct(theme_dir, is_custom);

        // Cache the result (even if None)
        color_cache.set(dir_name.clone(), colors.clone()).await;
        log::debug!("Cached colors for theme: {dir_name}");

        colors
    }

    /// Direct color extraction (moved from original implementation)
    fn extract_theme_colors_direct(theme_dir: &Path, is_custom: bool) -> Option<ThemeColors> {
        if is_custom {
            // For custom themes, try to extract from custom_theme.json
            let custom_theme_path = theme_dir.join("custom_theme.json");
            if custom_theme_path.exists() {
                match fs::read_to_string(&custom_theme_path) {
                    Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
                        Ok(theme_data) => {
                            if let Some(colors) =
                                ColorExtractor::extract_from_custom_theme(&theme_data)
                            {
                                return Some(colors);
                            }
                        },
                        Err(e) => {
                            log::warn!(
                                "Failed to parse custom theme JSON at {custom_theme_path:?}: {e}"
                            );
                        },
                    },
                    Err(e) => {
                        log::warn!(
                            "Failed to read custom theme file at {custom_theme_path:?}: {e}"
                        );
                    },
                }
            }
        }

        // For system themes or fallback, try to extract from alacritty.toml
        let alacritty_config_path = theme_dir.join("alacritty.toml");
        if alacritty_config_path.exists() {
            if let Some(colors) =
                ColorExtractor::extract_from_alacritty_config(&alacritty_config_path)
            {
                return Some(colors);
            }
        }

        None
    }

    /// Load theme image asynchronously
    async fn load_theme_image_async(theme_dir: &Path) -> String {
        // This is I/O bound, so we can spawn it as a blocking task
        let theme_dir_path = theme_dir.to_path_buf();
        let theme_dir_display = theme_dir.display().to_string();

        match tokio::task::spawn_blocking(move || Self::find_and_convert_image(&theme_dir_path))
            .await
        {
            Ok(Ok(image_path)) => image_path,
            Ok(Err(e)) => {
                log::warn!("Failed to load image for theme {theme_dir_display}: {e}");
                String::new()
            },
            Err(e) => {
                log::warn!("Image loading task failed for theme {theme_dir_display}: {e}");
                String::new()
            },
        }
    }

    /// Find and convert image to data URL (blocking operation)
    fn find_and_convert_image(theme_dir: &Path) -> Result<String, String> {
        if let Ok(entries) = fs::read_dir(theme_dir) {
            for entry in entries.flatten() {
                let file_path = entry.path();
                if file_path.is_file() {
                    if let Some(extension) = file_path.extension().and_then(|ext| ext.to_str()) {
                        let ext_lower = extension.to_lowercase();
                        if matches!(
                            ext_lower.as_str(),
                            "png" | "jpg" | "jpeg" | "webp" | "gif" | "svg"
                        ) {
                            return Self::convert_image_to_data_url(&file_path);
                        }
                    }
                }
            }
        }
        Ok(String::new())
    }

    /// Convert a local image file to a base64 data URL
    fn convert_image_to_data_url(image_path: &Path) -> Result<String, String> {
        if !image_path.exists() {
            return Err(format!("Image file does not exist: {image_path:?}"));
        }

        let image_data =
            fs::read(image_path).map_err(|e| format!("Failed to read image file: {e}"))?;

        // Determine MIME type based on file extension
        let mime_type = match image_path.extension().and_then(|ext| ext.to_str()) {
            Some("png") => "image/png",
            Some("jpg") | Some("jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            Some("svg") => "image/svg+xml",
            _ => "image/png", // Default to PNG
        };

        let base64_data = Self::base64_encode(&image_data);
        Ok(format!("data:{mime_type};base64,{base64_data}"))
    }

    /// Optimized base64 encoding function with pre-allocated capacity
    fn base64_encode(data: &[u8]) -> String {
        if data.is_empty() {
            return String::new();
        }

        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

        // Pre-allocate with exact capacity to avoid reallocations
        let output_len = data.len().div_ceil(3) * 4;
        let mut result = String::with_capacity(output_len);

        for chunk in data.chunks(3) {
            let mut buf = [0u8; 3];
            for (i, &byte) in chunk.iter().enumerate() {
                buf[i] = byte;
            }

            let b = ((buf[0] as u32) << 16) | ((buf[1] as u32) << 8) | (buf[2] as u32);

            result.push(CHARS[((b >> 18) & 63) as usize] as char);
            result.push(CHARS[((b >> 12) & 63) as usize] as char);
            result.push(if chunk.len() > 1 {
                CHARS[((b >> 6) & 63) as usize] as char
            } else {
                '='
            });
            result.push(if chunk.len() > 2 {
                CHARS[(b & 63) as usize] as char
            } else {
                '='
            });
        }

        result
    }

    /// Clear the color cache
    pub async fn clear_cache(&self) {
        self.color_cache.clear().await;
        log::info!("Color extraction cache cleared");
    }

    /// Get cache statistics
    pub async fn get_cache_stats(&self) -> (usize,) {
        let size = self.color_cache.size().await;
        (size,)
    }
}

impl Default for OptimizedThemeLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_color_cache() {
        let cache = ColorCache::new();

        // Test empty cache
        assert!(cache.get("test").await.is_none());
        assert_eq!(cache.size().await, 0);

        // Test caching colors
        let colors = ColorExtractor::get_fallback_colors();
        cache.set("test".to_string(), Some(colors.clone())).await;

        assert_eq!(cache.size().await, 1);
        let cached = cache.get("test").await.unwrap().unwrap();
        assert_eq!(cached.primary.background, colors.primary.background);

        // Test caching None
        cache.set("empty".to_string(), None).await;
        assert_eq!(cache.size().await, 2);
        assert!(cache.get("empty").await.unwrap().is_none());

        // Test cache clear
        cache.clear().await;
        assert_eq!(cache.size().await, 0);
    }

    #[tokio::test]
    async fn test_generate_theme_metadata() {
        let temp_dir = TempDir::new().unwrap();
        let theme_dir = temp_dir.path().join("test-theme");
        fs::create_dir(&theme_dir).unwrap();

        // Create a custom theme file
        let custom_theme_data = json!({
            "alacritty": {
                "colors": {
                    "primary": {
                        "background": "#121212",
                        "foreground": "#bebebe"
                    }
                }
            }
        });
        fs::write(
            theme_dir.join("custom_theme.json"),
            custom_theme_data.to_string(),
        )
        .unwrap();

        // Create an image file
        fs::write(theme_dir.join("preview.png"), b"fake image data").unwrap();

        let metadata = OptimizedThemeLoader::generate_theme_metadata(&theme_dir)
            .await
            .unwrap();

        assert_eq!(metadata.dir, "test-theme");
        assert_eq!(metadata.title, "Test Theme");
        assert!(metadata.is_custom);
        assert!(!metadata.is_system);
        assert!(metadata.has_colors);
        assert!(metadata.has_image);
    }

    #[tokio::test]
    async fn test_extract_theme_colors_cached() {
        let temp_dir = TempDir::new().unwrap();
        let theme_dir = temp_dir.path().join("cached-theme");
        fs::create_dir(&theme_dir).unwrap();

        // Create alacritty config with complete color scheme
        let alacritty_config = "[colors.primary]\nbackground = \"#1a1a1a\"\nforeground = \"#ffffff\"\n\n[colors.normal]\nblack = \"#000000\"\nred = \"#ff5555\"\ngreen = \"#50fa7b\"\nyellow = \"#f1fa8c\"\nblue = \"#8be9fd\"\nmagenta = \"#ff79c6\"\ncyan = \"#8be9fd\"\nwhite = \"#ffffff\"";
        fs::write(theme_dir.join("alacritty.toml"), alacritty_config).unwrap();

        let cache = ColorCache::new();

        // First call should extract and cache
        let colors1 =
            OptimizedThemeLoader::extract_theme_colors_cached(&theme_dir, false, &cache).await;
        assert!(colors1.is_some());
        assert_eq!(cache.size().await, 1);

        // Second call should use cache
        let colors2 =
            OptimizedThemeLoader::extract_theme_colors_cached(&theme_dir, false, &cache).await;
        assert!(colors2.is_some());
        assert_eq!(cache.size().await, 1); // Size shouldn't change

        // Colors should be the same
        let c1 = colors1.unwrap();
        let c2 = colors2.unwrap();
        assert_eq!(c1.primary.background, c2.primary.background);
    }

    #[test]
    fn test_has_image_files() {
        let temp_dir = TempDir::new().unwrap();
        let theme_dir = temp_dir.path().join("image-test");
        fs::create_dir(&theme_dir).unwrap();

        // No images initially
        assert!(!OptimizedThemeLoader::has_image_files(&theme_dir));

        // Add a PNG file
        fs::write(theme_dir.join("preview.png"), b"fake png").unwrap();
        assert!(OptimizedThemeLoader::has_image_files(&theme_dir));

        // Add a non-image file
        let theme_dir2 = temp_dir.path().join("no-image-test");
        fs::create_dir(&theme_dir2).unwrap();
        fs::write(theme_dir2.join("config.toml"), b"config data").unwrap();
        assert!(!OptimizedThemeLoader::has_image_files(&theme_dir2));
    }

    #[test]
    fn test_convert_image_to_data_url() {
        let temp_dir = TempDir::new().unwrap();
        let image_path = temp_dir.path().join("test.png");
        fs::write(&image_path, b"fake png data").unwrap();

        let result = OptimizedThemeLoader::convert_image_to_data_url(&image_path).unwrap();
        assert!(result.starts_with("data:image/png;base64,"));
        assert!(result.len() > 30); // Should have base64 encoded data
    }

    #[test]
    fn test_base64_encode() {
        let data = b"hello world";
        let encoded = OptimizedThemeLoader::base64_encode(data);
        assert_eq!(encoded, "aGVsbG8gd29ybGQ=");

        let empty_data = b"";
        let empty_encoded = OptimizedThemeLoader::base64_encode(empty_data);
        assert_eq!(empty_encoded, "");
    }
}

#[tokio::test]
async fn test_metadata_loading_performance() {
    // Test that metadata loading works correctly
    let loader = OptimizedThemeLoader::new();

    let result = loader.load_theme_metadata_only().await;

    match result {
        Ok(metadata) => {
            // Verify metadata structure if themes exist
            for meta in metadata {
                assert!(!meta.dir.is_empty());
                assert!(!meta.title.is_empty());
            }
        },
        Err(e) => {
            assert!(e.contains("Themes directory does not exist"));
        },
    }
}

#[tokio::test]
async fn test_cache_statistics() {
    let loader = OptimizedThemeLoader::new();

    // Initially cache should be empty
    let (cache_size,) = loader.get_cache_stats().await;
    assert_eq!(cache_size, 0);

    // Add something to cache
    let cache = &loader.color_cache;
    let colors = ColorExtractor::get_fallback_colors();
    cache.set("test-theme".to_string(), Some(colors)).await;

    // Cache size should increase
    let (cache_size,) = loader.get_cache_stats().await;
    assert_eq!(cache_size, 1);

    // Clear cache
    loader.clear_cache().await;

    // Cache should be empty again
    let (cache_size,) = loader.get_cache_stats().await;
    assert_eq!(cache_size, 0);
}

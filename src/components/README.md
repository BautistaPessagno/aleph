# Icon Component

The Icon component provides local icon support with emoji fallbacks for the Aleph application.

## Features

- **Local SVG Icons**: Uses local SVG files for crisp, scalable icons
- **Emoji Fallbacks**: Automatically falls back to emojis if SVG icons fail to load
- **TypeScript Support**: Fully typed with IconType definitions
- **Customizable**: Supports size, color, and className props
- **Extensible**: Easy to add new icon types

## Usage

```tsx
import Icon from './components/Icon';

// Basic usage
<Icon type="file" />

// With custom size and styling
<Icon type="app" size={24} className="my-icon" />

// With color (applies CSS filter to SVG)
<Icon type="folder" color="#007aff" />

// With custom fallback emoji
<Icon type="custom" fallbackEmoji="🎯" />
```

## Available Icon Types

- `folder` - Folder icon
- `file` - Generic file icon  
- `app` - Application icon
- `image` - Image file icon
- `video` - Video file icon
- `audio` - Audio file icon
- `archive` - Archive/compressed file icon
- `code` - Code file icon
- `pdf` - PDF file icon
- `text` - Text file icon
- `search` - Search icon

## Adding New Icons

1. Add your SVG file to `src/assets/icons/`
2. Import it in `src/components/Icon.tsx`
3. Add the type to the `IconType` union
4. Add mappings in `iconMap` and `emojiMap`

## Icon Files Location

All icon SVG files are stored in `src/assets/icons/`:

- `folder.svg`
- `file.svg`  
- `app.svg`
- `image.svg`
- `video.svg`
- `audio.svg`
- `archive.svg`
- `code.svg`
- `pdf.svg`
- `search.svg`

## Fallback System

The component includes a robust fallback system:

1. **Primary**: Local SVG icon
2. **Secondary**: Emoji fallback if SVG fails to load
3. **Tertiary**: Custom emoji if provided via `fallbackEmoji` prop

This ensures icons always display, even if SVG files are missing or fail to load.
import React from 'react';

// Import all icon SVGs
import folderIcon from '../assets/icons/folder.svg';
import fileIcon from '../assets/icons/file.svg';
import appIcon from '../assets/icons/app.svg';
import imageIcon from '../assets/icons/image.svg';
import videoIcon from '../assets/icons/video.svg';
import audioIcon from '../assets/icons/audio.svg';
import archiveIcon from '../assets/icons/archive.svg';
import codeIcon from '../assets/icons/code.svg';
import pdfIcon from '../assets/icons/pdf.svg';
import searchIcon from '../assets/icons/search.svg';

export type IconType = 
  | 'folder' 
  | 'file' 
  | 'app' 
  | 'image' 
  | 'video' 
  | 'audio' 
  | 'archive' 
  | 'code' 
  | 'pdf'
  | 'text'
  | 'search';

interface IconProps {
  type: IconType;
  size?: number;
  className?: string;
  color?: string;
  fallbackEmoji?: string;
}

// Icon mapping
const iconMap: Record<IconType, string> = {
  folder: folderIcon,
  file: fileIcon,
  app: appIcon,
  image: imageIcon,
  video: videoIcon,
  audio: audioIcon,
  archive: archiveIcon,
  code: codeIcon,
  pdf: pdfIcon,
  text: fileIcon, // Use generic file icon for text files
  search: searchIcon,
};

// Fallback emoji mapping
const emojiMap: Record<IconType, string> = {
  folder: "📁",
  file: "📄",
  app: "🚀",
  image: "🖼️",
  video: "🎬",
  audio: "🎵",
  archive: "📦",
  code: "💻",
  pdf: "📕",
  text: "📄",
  search: "🔍",
};

export const Icon: React.FC<IconProps> = ({ 
  type, 
  size = 20, 
  className = '', 
  color,
  fallbackEmoji 
}) => {
  const iconSrc = iconMap[type];
  const emoji = fallbackEmoji || emojiMap[type];

  // If we have an SVG icon, use it
  if (iconSrc) {
    return (
      <img
        src={iconSrc}
        alt={`${type} icon`}
        width={size}
        height={size}
        className={`icon ${className}`}
        style={{ 
          color: color,
          filter: color ? `brightness(0) saturate(100%) ${getColorFilter(color)}` : undefined
        }}
        onError={(e) => {
          // Fallback to emoji if SVG fails to load
          const target = e.target as HTMLImageElement;
          target.style.display = 'none';
          const emojiSpan = document.createElement('span');
          emojiSpan.textContent = emoji;
          emojiSpan.style.fontSize = `${size}px`;
          emojiSpan.className = `icon-emoji ${className}`;
          target.parentNode?.insertBefore(emojiSpan, target);
        }}
      />
    );
  }

  // Fallback to emoji
  return (
    <span 
      className={`icon-emoji ${className}`}
      style={{ fontSize: `${size}px` }}
    >
      {emoji}
    </span>
  );
};

// Helper function to convert color to CSS filter (approximate)
function getColorFilter(color: string): string {
  // This is a simplified approach - for more accurate color conversion,
  // you might want to use a more sophisticated color conversion library
  const colorMap: Record<string, string> = {
    '#ffffff': 'invert(100%)',
    '#000000': 'invert(0%)',
    '#007aff': 'invert(27%) sepia(96%) saturate(1352%) hue-rotate(207deg) brightness(97%) contrast(103%)',
    // Add more color mappings as needed
  };
  
  return colorMap[color.toLowerCase()] || '';
}

export default Icon;
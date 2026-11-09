import { defineEcConfig } from '@astrojs/starlight/expressive-code';

// Code blocks use GitHub's syntax themes, its pale panel and border, and 6px
// corners. Shell examples keep their window frame with its three dots, and a
// block that names a file gets its tab. Colours that differ between light and
// dark are functions of the theme, which is why this lives in its own file.
const dark = (theme) => theme.type === 'dark';

export default defineEcConfig({
  themes: ['github-dark-default', 'github-light-default'],
  styleOverrides: {
    borderRadius: '6px',
    borderWidth: '1px',
    borderColor: ({ theme }) => (dark(theme) ? '#3d444d' : '#d1d9e0'),
    codeBackground: ({ theme }) => (dark(theme) ? '#151b23' : '#f6f8fa'),
    codeFontFamily: 'var(--font-mono)',
    codeFontSize: '0.75rem',
    codeLineHeight: '1.6',
    codePaddingBlock: '1rem',
    codePaddingInline: '1rem',
    uiFontFamily: 'var(--font-body)',
    frames: {
      shadowColor: 'transparent',
      frameBoxShadowCssValue: 'none',
      editorBackground: ({ theme }) => (dark(theme) ? '#151b23' : '#f6f8fa'),
      editorTabBarBackground: ({ theme }) => (dark(theme) ? '#0d1117' : '#ffffff'),
      editorActiveTabBackground: ({ theme }) => (dark(theme) ? '#151b23' : '#f6f8fa'),
      editorActiveTabIndicatorTopColor: 'transparent',
      editorTabBarBorderBottomColor: ({ theme }) => (dark(theme) ? '#3d444d' : '#d1d9e0'),
      terminalBackground: ({ theme }) => (dark(theme) ? '#151b23' : '#f6f8fa'),
      terminalTitlebarBackground: ({ theme }) => (dark(theme) ? '#0d1117' : '#eef1f4'),
      terminalTitlebarBorderBottomColor: ({ theme }) => (dark(theme) ? '#3d444d' : '#d1d9e0'),
      terminalTitlebarDotsForeground: ({ theme }) => (dark(theme) ? '#3d444d' : '#c8d1da'),
    },
  },
});

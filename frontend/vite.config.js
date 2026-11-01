import { fileURLToPath, URL } from 'node:url'

import path from 'path'
import { defineConfig, loadEnv } from 'vite'
import vue from '@vitejs/plugin-vue'

import UnoCSS from 'unocss/vite'

import Icons from 'unplugin-icons/vite'
import IconsResolver from 'unplugin-icons/resolver'
import AutoImport from 'unplugin-auto-import/vite'
import Components from 'unplugin-vue-components/vite'
import { ElementPlusResolver } from 'unplugin-vue-components/resolvers'
import { visualizer } from "rollup-plugin-visualizer";
// import viteImagemin from 'vite-plugin-imagemin'
// import { chunkSplitPlugin } from 'vite-plugin-chunk-split'

const pathSrc = path.resolve(__dirname, 'src')

function manualChunks(id) {
  console.log(id);
  if (id.includes('antv')) {
    return 'antv';
  } else {
    return 'index';
  }
}

// https://vitejs.dev/config/
export default ({ command, mode }) => {
  return defineConfig({
    // 开发服务器代理：浏览器始终访问同源的 `/api/*`，由 Vite 转给 Rust 后端。
    // 这样 HttpOnly cookie 不需要 CORS，也不会再出现登录 POST 的 OPTIONS 预检 405。
    // 生产构建仍然走同源 `/`（见 .env.production），不经过这个代理。
    server: {
      proxy: {
        '/api': {
          target: 'http://127.0.0.1:12715',
          changeOrigin: true,
          rewrite: (path) => path.replace(/^\/api/, ''),
        },
      },
    },
    // resolve: {
    //   alias: {
    //     '@': pathSrc,
    //   },
    // },
    resolve: {
      alias: {
        '@': fileURLToPath(new URL('./src', import.meta.url))
      }
    },
    plugins: [
      UnoCSS(),
      vue(),
      // visualizer(),
      AutoImport({
        resolvers: [
          ElementPlusResolver(),
          IconsResolver({
            prefix: 'Icon',
          }),
        ],
        dts: path.resolve(pathSrc, 'auto-imports.d.ts'),
      }),
      Components({
        resolvers: [
          ElementPlusResolver(),
          IconsResolver({
            enabledCollections: ['ep'],
          }),
        ],
        dts: path.resolve(pathSrc, 'components.d.ts'),
      }),
      Icons({
        autoInstall: true,
      }),
      // viteImagemin({
      //   gifsicle: {
      //     optimizationLevel: 7,
      //     interlaced: false,
      //   },
      //   optipng: {
      //     optimizationLevel: 7,
      //   },
      //   mozjpeg: {
      //     quality: 70,
      //   },
      //   pngquant: {
      //     quality: [0.8, 0.9],
      //     speed: 4,
      //   },
      //   svgo: {
      //     plugins: [
      //       {
      //         name: 'removeViewBox',
      //       },
      //       {
      //         name: 'removeEmptyAttrs',
      //         active: false,
      //       },
      //     ],
      //   },
      // }),
      // chunkSplitPlugin({
      //   customSplitting: {
      //     'canvas': [/\/node_modules\/@antv\//]
      //   }
      // }),
    ],
    define: {
      // 'process.env': env,
    },
    build: {
      rollupOptions: {
        output: {
          // dir: 'dist',
          // entryFileNames: 'index.js',
          // manualChunks,
          // chunkFilename: 'vendor_locale_[name].js',
          // manualChunks: {
          //   'canvas': ['@antv/x6', '@antv/x6-vue-shape']
          // }
        },
      },
      minify: 'esbuild', // <-- add
      terserOptions: {
        compress: {
          drop_console: true,
          drop_debugger: true,
        },
      },
    },
    esbuild: {
      drop: ['console', 'debugger'],
    },
  })
}

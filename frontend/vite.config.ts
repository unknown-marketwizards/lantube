import {defineConfig} from 'vite'
import vue from '@vitejs/plugin-vue'

// https://vitejs.dev/config/
export default defineConfig({
    plugins: [
        vue()
    ],
    define: {'process.env': {}},
    server: {
        proxy: {
            '/api': {
                target: 'http://127.0.0.1:8888',
                changeOrigin: true,
                rewrite: (path) => path.replace(/^\/api/, '/api')
            },
            '/file': {
                target: 'http://127.0.0.1:8888',
                changeOrigin: true,
                pathRewrite: {
                    '^/file': '/file'
                }
            }
        }
    },
    /* remove the need to specify .vue files https://vitejs.dev/config/#resolve-extensions
    resolve: {
      extensions: [
        '.js',
        '.json',
        '.jsx',
        '.mjs',
        '.ts',
        '.tsx',
        '.vue',
      ]
    },
    */
})

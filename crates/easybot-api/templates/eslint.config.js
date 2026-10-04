// 宽松 ESLint 配置：仅捕获真正的代码缺陷。
// 注意：本项目前端沿用「无 bundler + 跨文件全局作用域」架构
// （utils.js / admin.js 由 build.rs 拼接进同一 <script>），
// 且部分函数由 HTML 内联 onclick 调用，因此关闭 no-undef / no-unused-vars，
// 避免大量误报。规则副作用其余保持开启。
import globals from 'globals';

const RULES = {
  'no-redeclare': 'error',
  'no-dupe-keys': 'error',
  'no-dupe-args': 'error',
  'no-unreachable': 'error',
  'no-cond-assign': 'error',
  'no-constant-condition': 'error',
  'no-obj-calls': 'error',
  'no-self-assign': 'error',
  'use-isnan': 'error',
  'eqeqeq': 'error',
};

export default [
  { ignores: ['node_modules/**', 'gen/**'] },
  {
    files: ['js/**/*.js'],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: 'script',
      globals: { ...globals.browser },
    },
    rules: { ...RULES, 'no-undef': 'off', 'no-unused-vars': 'off' },
  },
  {
    files: ['tests/**/*.js'],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: 'module',
      globals: { ...globals.node, ...globals.browser },
    },
    rules: RULES,
  },
];

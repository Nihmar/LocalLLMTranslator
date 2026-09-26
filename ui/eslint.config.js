import js from "@eslint/js";
import globals from "globals";
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

// Flat ESLint config. Kept dependency-light and fully offline: the TypeScript compiler is the
// real type gate (`npm run typecheck`), ESLint only enforces style and common mistakes.
//
// Only the two classic hook rules are enabled. The full `react-hooks` v7 "recommended" preset
// also ships the React Compiler rule set, whose `set-state-in-effect` rule rejects the ordinary
// and correct "load data when the view mounts" effect used by every view here; adopting it would
// mean restructuring working code to satisfy a rule aimed at a compiler this project does not
// use. `rules-of-hooks` and `exhaustive-deps` are the ones that catch real defects.
export default tseslint.config(
  {
    ignores: ["dist/**", "node_modules/**"],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["**/*.{ts,tsx}"],
    plugins: {
      "react-hooks": reactHooks,
    },
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      globals: {
        ...globals.browser,
      },
    },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "error",
      "@typescript-eslint/consistent-type-imports": [
        "error",
        { prefer: "type-imports", fixStyle: "inline-type-imports" },
      ],
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
      eqeqeq: ["error", "always"],
      "no-var": "error",
      "prefer-const": "error",
      "no-console": ["warn", { allow: ["warn", "error"] }],
    },
  },
);

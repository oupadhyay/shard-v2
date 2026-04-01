import fs from 'fs';

const filePath = 'src/ui/markdown.ts';
let content = fs.readFileSync(filePath, 'utf8');

content = content.replace(
  `/** Clear KaTeX errors before a new render */
/** Test helper to populate KaTeX errors */`,
  `/** Test helper to populate KaTeX errors */`
);

content = content.replace(
  `export function clearKatexErrors(): void {`,
  `/** Clear KaTeX errors before a new render */
export function clearKatexErrors(): void {`
);

fs.writeFileSync(filePath, content, 'utf8');

const fs = require('fs');
const path = require('path');

const srcDir = path.join(__dirname, '..', 'src');
const distDir = path.join(__dirname, '..', 'dist');

if (fs.existsSync(distDir)) {
    fs.rmSync(distDir, { recursive: true, force: true });
}
fs.mkdirSync(distDir, { recursive: true });

function copyDir(src, dest) {
    const entries = fs.readdirSync(src, { withFileTypes: true });
    for (const entry of entries) {
        const srcPath = path.join(src, entry.name);
        const destPath = path.join(dest, entry.name);
        if (entry.isDirectory()) {
            fs.mkdirSync(destPath, { recursive: true });
            copyDir(srcPath, destPath);
        } else {
            fs.copyFileSync(srcPath, destPath);
        }
    }
}

copyDir(srcDir, distDir);

// Copy @tauri-apps/api into dist/api (Tauri blocks node_modules in frontendDist)
const apiSrc = path.join(__dirname, '..', 'node_modules', '@tauri-apps', 'api');
const apiDest = path.join(distDir, 'api');
fs.mkdirSync(apiDest, { recursive: true });
copyDir(apiSrc, apiDest);

console.log(`Copied ${srcDir} -> ${distDir}`);

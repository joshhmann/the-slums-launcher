const { execSync } = require('child_process');
const path = require('path');
const os = require('os');

if (os.platform() !== 'linux') {
    console.log('AppImage patch skipped — only needed on Linux');
    process.exit(0);
}

const script = path.join(__dirname, 'patch-appimage.sh');
try {
    execSync(`bash "${script}"`, { stdio: 'inherit' });
} catch (e) {
    console.error('AppImage patch failed (non-fatal):', e.message);
}

// Convert the two AM8 Pro preset arrays from the user's installed Genie into JSON.
import fs from 'node:fs';
import vm from 'node:vm';

const source = fs.readFileSync('C:\\Program Files\\fifine Genie\\fifine Genie\\resources\\app\\dist\\assets\\index-Cibzib1Q.js', 'utf8');
function extract(name) {
    const start = source.indexOf(`${name}=[`);
    if (start < 0) throw new Error(`Missing ${name}`);
    const arrayStart = start + name.length + 1;
    let depth = 0, quote = false, escape = false;
    for (let i = arrayStart; i < source.length; i++) {
        const c = source[i];
        if (quote) {
            if (escape) escape = false;
            else if (c === '\\') escape = true;
            else if (c === '"') quote = false;
        } else if (c === '"') quote = true;
        else if (c === '[') depth++;
        else if (c === ']' && --depth === 0) {
            return vm.runInNewContext(`(${source.slice(arrayStart, i + 1)})`, Object.create(null), {timeout: 1000});
        }
    }
    throw new Error(`Unterminated ${name}`);
}
fs.mkdirSync('data', {recursive: true});
const presets = {output: extract('II'), microphone: extract('LI')};
fs.writeFileSync('data/presets.json', JSON.stringify(presets, null, 2) + '\n');
console.log(`Extracted ${presets.output.length} output and ${presets.microphone.length} microphone presets`);

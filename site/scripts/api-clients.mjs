// The API examples in languages other than curl. Each writer takes the same
// description of a request and returns a complete program, so every example
// for an operation sends the same thing:
//   method   'POST'
//   url      the address, where $NAME stands for an environment variable
//   headers  [name, value] pairs
//   form     multipart fields: { name, file } uploads a file, { name, text }
//            sends a file's contents as text, { name, value } sends a value
//   json     a JSON body, or jsonFile, the file that holds one
//   reply    what comes back: 'file' (saved to out), 'json', 'text' or 'none'

class Code {
  constructor(text) {
    this.text = text;
  }
}
class Call {
  constructor(name, ...args) {
    this.name = name;
    this.args = args;
  }
}
const code = (text) => new Code(text);

// 'templates/$ID/content' as its text and its variables, in turn.
function pieces(text) {
  const found = [];
  let from = 0;
  for (const match of text.matchAll(/\$([A-Z_]+)/g)) {
    if (match.index > from) found.push({ text: text.slice(from, match.index) });
    found.push({ variable: match[1] });
    from = match.index + match[0].length;
  }
  if (from < text.length || !found.length) found.push({ text: text.slice(from) });
  return found;
}
const only = (parts) => (parts.length === 1 ? parts[0].variable : undefined);

// A form value that is itself JSON reads better built from an object.
function parsed(value) {
  if (value[0] !== '{' && value[0] !== '[') return undefined;
  try {
    return JSON.parse(value);
  } catch {
    return undefined;
  }
}

// Writes values the way a formatter would: on one line while that fits, else
// an entry a line.
function printer({ string, literal, key, spaced, step, width }) {
  const print = (value, indent = 0, col = indent) => {
    if (value instanceof Code) return value.text;
    if (value instanceof Call) {
      let at = col + value.name.length + 1;
      const args = value.args.map((arg) => {
        const text = print(arg, indent, at);
        at += text.length + 2;
        return text;
      });
      return `${value.name}(${args.join(', ')})`;
    }
    if (typeof value === 'string') return string(value);
    if (value === null || typeof value !== 'object') return literal(value);
    const array = Array.isArray(value);
    const items = array ? value.map((item) => ['', item]) : Object.entries(value).map(([k, item]) => [`${key(k)}: `, item]);
    const [open, close] = array ? '[]' : '{}';
    if (!items.length) return open + close;
    const pad = spaced && !array ? ' ' : '';
    const flat = open + pad + items.map(([k, item]) => k + print(item, 0, -Infinity)).join(', ') + pad + close;
    if (!flat.includes('\n') && col + flat.length + 2 <= width) return flat;
    const inner = indent + step;
    const lines = items.map(([k, item]) => `${' '.repeat(inner)}${k}${print(item, inner, inner + k.length)},`);
    return `${open}\n${lines.join('\n')}\n${' '.repeat(indent)}${close}`;
  };
  return print;
}

/* Node.js */

function jsString(text) {
  const parts = pieces(text);
  if (only(parts)) return `process.env.${only(parts)}`;
  if (parts.some((part) => part.variable)) {
    const inside = parts.map((part) => (part.variable ? `\${process.env.${part.variable}}` : part.text.replace(/[`\\]|\$\{/g, '\\$&')));
    return `\`${inside.join('')}\``;
  }
  return `'${text.replace(/[\\']/g, '\\$&')}'`;
}
const js = printer({
  string: jsString,
  literal: String,
  key: (name) => (/^[A-Za-z_$][\w$]*$/.test(name) ? name : jsString(name)),
  spaced: true,
  step: 2,
  width: 80,
});

function node(request) {
  const lines = [];
  if (request.form) {
    lines.push('const form = new FormData();');
    for (const field of request.form) {
      const start = `form.append(${jsString(field.name)}, `;
      let value;
      if (field.file) value = `await openAsBlob(${jsString(field.file)}), ${jsString(field.file)}`;
      else if (field.text) value = `await readFile(${jsString(field.text)}, 'utf8')`;
      else {
        const json = parsed(field.value);
        value = js(json ? new Call('JSON.stringify', json) : field.value, 0, start.length);
      }
      lines.push(`${start}${value});`);
    }
    lines.push('');
  }

  const headers = Object.fromEntries(request.headers);
  const options = {};
  if (request.method !== 'GET') options.method = request.method;
  if (request.json || request.jsonFile) headers['Content-Type'] = 'application/json';
  if (Object.keys(headers).length) options.headers = headers;
  if (request.form) options.body = code('form');
  if (request.jsonFile) options.body = code(`await readFile(${jsString(request.jsonFile)})`);
  if (request.json) options.body = new Call('JSON.stringify', request.json);

  const start = `const response = await fetch(${jsString(request.url)}`;
  lines.push(Object.keys(options).length ? `${start}, ${js(options, 0, start.length + 2)});` : `${start});`);
  lines.push('if (!response.ok) throw new Error(await response.text());');
  if (request.reply === 'file') lines.push(`await writeFile(${jsString(request.out)}, Buffer.from(await response.arrayBuffer()));`);
  if (request.reply === 'json') lines.push('console.log(await response.json());');
  if (request.reply === 'text') lines.push('console.log(await response.text());');

  const program = lines.join('\n');
  const promises = ['readFile', 'writeFile'].filter((name) => program.includes(`${name}(`));
  const imports = [];
  if (program.includes('openAsBlob(')) imports.push("import { openAsBlob } from 'node:fs';");
  if (promises.length) imports.push(`import { ${promises.join(', ')} } from 'node:fs/promises';`);
  return (imports.length ? `${imports.join('\n')}\n\n` : '') + program;
}

/* Python */

function pyString(text) {
  const parts = pieces(text);
  if (only(parts)) return `os.environ["${only(parts)}"]`;
  if (parts.some((part) => part.variable)) {
    const inside = parts.map((part) =>
      part.variable ? `{os.environ['${part.variable}']}` : part.text.replace(/[\\"]/g, '\\$&').replace(/[{}]/g, '$&$&'),
    );
    return `f"${inside.join('')}"`;
  }
  return text.includes('"') && !/['\\]/.test(text) ? `'${text}'` : JSON.stringify(text);
}
const py = printer({
  string: pyString,
  literal: (value) => (value === null ? 'None' : value === true ? 'True' : value === false ? 'False' : String(value)),
  key: pyString,
  spaced: false,
  step: 4,
  width: 88,
});

function python(request) {
  const args = [[null, request.url]];
  const headers = Object.fromEntries(request.headers);
  if (request.jsonFile) headers['Content-Type'] = 'application/json';
  if (Object.keys(headers).length) args.push(['headers', headers]);

  let note;
  if (request.form) {
    const value = (field) => {
      if (field.text) return code(`Path(${pyString(field.text)}).read_text()`);
      const json = parsed(field.value);
      return json ? new Call('json.dumps', json) : field.value;
    };
    const files = request.form.filter((field) => field.file);
    const fields = request.form.filter((field) => !field.file);
    if (new Set(fields.map((field) => field.name)).size !== fields.length) throw new Error('a form field is repeated');
    if (files.length) {
      args.push(['files', files.map((field) => new Call('', field.name, code(`open(${pyString(field.file)}, "rb")`)))]);
      if (fields.length) args.push(['data', Object.fromEntries(fields.map((field) => [field.name, value(field)]))]);
    } else {
      note = '# A (None, value) pair sends a plain field in a multipart form.';
      args.push(['files', Object.fromEntries(fields.map((field) => [field.name, new Call('', code('None'), value(field))]))]);
    }
  }
  if (request.json) args.push(['json', request.json]);
  if (request.jsonFile) args.push(['data', code(`Path(${pyString(request.jsonFile)}).read_bytes()`)]);

  const start = `response = requests.${request.method.toLowerCase()}(`;
  const named = ([name]) => (name ? `${name}=` : '');
  const flat = start + args.map((arg) => named(arg) + py(arg[1], 0, -Infinity)).join(', ') + ')';
  const lines = note ? [note] : [];
  if (flat.length <= 88) lines.push(flat);
  else lines.push(start, ...args.map((arg) => `    ${named(arg)}${py(arg[1], 4, 4 + named(arg).length)},`), ')');
  lines.push('response.raise_for_status()');
  if (request.reply === 'file') lines.push(`Path(${pyString(request.out)}).write_bytes(response.content)`);
  if (request.reply === 'json') lines.push('print(response.json())');
  if (request.reply === 'text') lines.push('print(response.text)');

  const program = lines.join('\n');
  const imports = [];
  if (program.includes('json.dumps(')) imports.push('import json');
  if (program.includes('os.environ')) imports.push('import os');
  if (program.includes('Path(')) imports.push('from pathlib import Path');
  return `${imports.length ? `${imports.join('\n')}\n\n` : ''}import requests\n\n${program}`;
}

/* Go */

function goString(text) {
  const plain = (part) => (part.includes('"') && !part.includes('`') ? `\`${part}\`` : JSON.stringify(part));
  return pieces(text)
    .map((part) => (part.variable ? `os.Getenv("${part.variable}")` : plain(part.text)))
    .join(' + ');
}

const GO_NAMES = ['body', 'form', 'url', 'request', 'response', 'message', 'out', 'err'];
const GO_PACKAGES = { bytes: 'bytes', io: 'io', log: 'log', multipart: 'mime/multipart', http: 'net/http', os: 'os', filepath: 'path/filepath', strings: 'strings' };
const GO_ATTACH = `
// attach adds the file at path to the form as the named field.
func attach(form *multipart.Writer, field, path string) {
	file, err := os.Open(path)
	if err != nil {
		log.Fatal(err)
	}
	defer file.Close()
	part, err := form.CreateFormFile(field, filepath.Base(path))
	if err != nil {
		log.Fatal(err)
	}
	if _, err := io.Copy(part, file); err != nil {
		log.Fatal(err)
	}
}`;

function go(request) {
  const failed = ['if err != nil {', '\tlog.Fatal(err)', '}'];
  const main = [];
  let body = 'nil';
  if (request.form) {
    main.push('var body bytes.Buffer', 'form := multipart.NewWriter(&body)');
    for (const field of request.form) {
      const name = goString(field.name);
      if (field.file) main.push(`attach(form, ${name}, ${goString(field.file)})`);
      else if (field.text) {
        if (GO_NAMES.includes(field.name) || !/^[a-z]\w*$/.test(field.name)) throw new Error(`cannot name a Go variable ${field.name}`);
        main.push(`${field.name}, err := os.ReadFile(${goString(field.text)})`, ...failed, `form.WriteField(${name}, string(${field.name}))`);
      } else main.push(`form.WriteField(${name}, ${goString(field.value)})`);
    }
    main.push('form.Close()', '');
    body = '&body';
  } else if (request.jsonFile) {
    main.push(`body, err := os.Open(${goString(request.jsonFile)})`, ...failed, 'defer body.Close()', '');
    body = 'body';
  } else if (request.json) {
    const short = JSON.stringify(request.json);
    main.push(`body := strings.NewReader(\`${short.length <= 56 ? short : JSON.stringify(request.json, null, 2)}\`)`, '');
    body = 'body';
  }

  let url = goString(request.url);
  if (url.includes(' + ')) {
    main.push(`url := ${url}`);
    url = 'url';
  }
  if (request.method === 'GET' && !request.headers.length) main.push(`response, err := http.Get(${url})`, ...failed);
  else {
    main.push(`request, err := http.NewRequest("${request.method}", ${url}, ${body})`, ...failed);
    for (const [name, value] of request.headers) main.push(`request.Header.Set(${goString(name)}, ${goString(value)})`);
    if (request.form) main.push('request.Header.Set("Content-Type", form.FormDataContentType())');
    if (request.json || request.jsonFile) main.push('request.Header.Set("Content-Type", "application/json")');
    main.push('', 'response, err := http.DefaultClient.Do(request)', ...failed);
  }
  main.push(
    'defer response.Body.Close()',
    'if response.StatusCode >= 300 {',
    '\tmessage, _ := io.ReadAll(response.Body)',
    '\tlog.Fatalf("%s: %s", response.Status, message)',
    '}',
  );
  const copy = (to) => [`if _, err := io.Copy(${to}, response.Body); err != nil {`, '\tlog.Fatal(err)', '}'];
  if (request.reply === 'file') main.push('', `out, err := os.Create(${goString(request.out)})`, ...failed, 'defer out.Close()', ...copy('out'));
  if (request.reply === 'json' || request.reply === 'text') main.push(...copy('os.Stdout'));

  const indented = [];
  let raw = false;
  for (const line of main.join('\n').split('\n')) {
    indented.push(line && !raw ? `\t${line}` : line);
    if ((line.match(/`/g) ?? []).length % 2) raw = !raw;
  }
  const program = `func main() {\n${indented.join('\n')}\n}${request.form?.some((field) => field.file) ? `\n${GO_ATTACH}` : ''}`;
  const used = Object.entries(GO_PACKAGES).filter(([name]) => new RegExp(`(^|[^\\w."/])${name}\\.`, 'm').test(program));
  return `package main\n\nimport (\n${used.map(([, path]) => `\t"${path}"`).join('\n')}\n)\n\n${program}`;
}

export const clients = [
  { lang: 'js', name: 'Node.js', write: node },
  { lang: 'python', name: 'Python', write: python },
  { lang: 'go', name: 'Go', write: go },
];

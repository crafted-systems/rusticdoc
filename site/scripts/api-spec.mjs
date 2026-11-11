// Prepares the OpenAPI description for the reference pages. public/openapi.json
// stays the service's own, byte for byte. The copy written here adds what only
// the site needs: a base URL, the overview's opening notes, and examples for
// every operation, in curl and in the languages api-clients.mjs writes. Each
// example is checked against the description, so one that names a field the
// service doesn't take stops the build.
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { clients } from './api-clients.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const BASE = 'http://localhost:9027';

const OVERVIEW = `

Requests are multipart forms unless a route says otherwise. Send credentials with \`X-Api-Key\` or \`Authorization: Bearer\`, or use basic auth, as [Authentication](/docs/guides/authentication/) explains. Failures are JSON, as [Errors](/docs/reference/errors/) describes.

The examples call a service on \`localhost:9027\` and send an API key in \`$KEY\`. In curl that is a shell variable; the Node.js, Python and Go examples read an environment variable of the same name. Leave the header out if yours has no credentials configured.

The same description is at [\`/openapi.json\`](/openapi.json) on this site and on your own service.`;

// What stands in for a path parameter, unless an example says otherwise.
const PARAMS = { id: '$ID', name: 'letter' };

// One or more examples per operationId:
//   label    the tab's name, when there are several
//   key      the variable holding the API key, when not $KEY
//   headers  extra request headers
//   form     multipart fields, as curl's -F takes them
//   json     a JSON body: an object, or '@file' to send a file
//   query    the query string
//   params   path parameters
//   method   the method, when curl would not pick it
//   out      the file the response is saved to
const SAMPLES = {
  health: [{}],
  ready: [{}],
  metrics: [{}],
  version: [{}],

  render: [
    {
      label: 'Upload a template',
      form: ['template=@invoice.docx', 'data=<invoice.json', 'steps=convert'],
      out: 'invoice.pdf',
    },
    {
      label: 'Stored template',
      json: { templateId: 'invoice', data: { customer: { name: 'Ada' }, total: 1500 }, steps: ['convert'] },
      out: 'invoice.pdf',
    },
    {
      label: 'Background job',
      headers: ['Prefer: respond-async'],
      form: ['template=@invoice.docx', 'data=<invoice.json', 'steps=convert'],
    },
    {
      label: 'Webhook',
      headers: ['X-Rusticdoc-Webhook-Url: https://receiver.example.com/documents'],
      form: ['template=@invoice.docx', 'data=<invoice.json', 'steps=convert'],
    },
  ],
  batch: [
    {
      label: 'Upload a template',
      form: [
        'template=@invoice.docx',
        'data=<invoices.json',
        'splitBy=invoices',
        'batchName=invoice-{{ item.number }}',
        'steps=convert',
      ],
      out: 'invoices.zip',
    },
    {
      label: 'Stored template',
      json: {
        templateId: 'invoice',
        data: { invoices: [{ number: 1001 }, { number: 1002 }] },
        splitBy: 'invoices',
        batchName: 'invoice-{{ item.number }}',
        steps: ['convert'],
      },
      out: 'invoices.zip',
    },
  ],
  validate: [
    { label: 'Upload a template', form: ['template=@letter.docx'] },
    { label: 'Stored template', json: { templateId: 'letter' } },
  ],

  convert: [
    { label: 'One file', form: ['files=@report.docx'], out: 'report.pdf' },
    {
      label: 'Several into one PDF/A',
      form: ['files=@cover.docx', 'files=@figures.xlsx', 'merge=true', 'pdfa=PDF/A-2b'],
      out: 'pack.pdf',
    },
  ],

  uploadTemplate: [{ form: ['template=@invoice.docx', 'name=invoice-2026'] }],
  listTemplates: [{ query: 'limit=20' }],
  getTemplate: [{}],
  deleteTemplate: [{ method: 'DELETE' }],
  downloadTemplate: [{ out: 'invoice.docx' }],
  templateVariables: [{}],
  draftVariables: [{ form: ['template=@invoice.docx'] }],
  previewDraft: [{ form: ['template=@invoice.docx', 'data=<invoice.json', 'width=400'], out: 'page.png' }],
  previewTemplate: [
    { label: 'Form', form: ['data=<invoice.json', 'width=400'], out: 'page.png' },
    { label: 'JSON', json: { data: { customer: { name: 'Ada' } }, width: 400 }, out: 'page.png' },
  ],
  listAliases: [{}],
  getAlias: [{}],
  setAlias: [
    { label: 'Form', method: 'PUT', form: ['templateId=$ID'] },
    { label: 'JSON', method: 'PUT', json: '@alias.json' },
  ],
  deleteAlias: [{ method: 'DELETE' }],

  getJob: [{ params: { id: '$JOB' } }],
  getJobResult: [{ params: { id: '$JOB' }, out: 'invoice.pdf' }],

  list: [{}],
  uploadFonts: [{ key: '$ADMIN_KEY', form: ['file=@brand-fonts.zip'] }],
  reload: [{ method: 'POST' }],
  deleteFont: [{ key: '$ADMIN_KEY', method: 'DELETE', params: { id: '$FONT' } }],

  merge: [{ form: ['files=@a.pdf', 'files=@b.pdf', 'outputFilename=both'], out: 'both.pdf' }],
  split: [{ form: ['files=@report.pdf', 'splitMode=intervals', 'splitSpan=1'], out: 'pages.zip' }],
  rotate: [{ form: ['files=@a.pdf', 'rotation=180'], out: 'turned.pdf' }],
  flatten: [{ form: ['files=@form.pdf'], out: 'flat.pdf' }],
  optimize: [{ form: ['files=@a.pdf'], out: 'small.pdf' }],
  encrypt: [{ form: ['files=@a.pdf', 'userPassword=open', 'ownerPassword=admin'], out: 'locked.pdf' }],
  convert_pdf: [{ form: ['files=@a.pdf', 'pdfa=PDF/A-2b'], out: 'archive.pdf' }],
  embed: [{ form: ['files=@a.pdf', 'files=@invoice.json'], out: 'attached.pdf' }],
  facturx: [{ form: ['files=@a.pdf', 'files=@factur-x.xml', 'profile=EN16931'], out: 'invoice.pdf' }],
  metadata_read: [{ form: ['files=@a.pdf'] }],
  metadata_write: [{ form: ['files=@a.pdf', 'metadata={"Title":"Invoice","Author":"Ada"}'], out: 'titled.pdf' }],
  bookmarks_read: [{ form: ['files=@a.pdf'] }],
  bookmarks_write: [{ form: ['files=@a.pdf', 'bookmarks=<bookmarks.json'], out: 'marked.pdf' }],
  watermark: [
    {
      form: ['files=@a.pdf', 'text=DRAFT', 'opacity=0.3', 'rotation=45', 'fontSize=48', 'colour=#ff0000'],
      out: 'draft.pdf',
    },
  ],
  stamp: [{ form: ['files=@a.pdf', 'files=@logo.png', 'position=top-right', 'scale=0.2'], out: 'branded.pdf' }],
  overlay: [{ form: ['files=@letter.pdf', 'files=@letterhead.pdf'], out: 'on-letterhead.pdf' }],
  sign: [
    {
      label: 'Sign',
      form: ['files=@contract.pdf', 'signature=certificate', 'reason=Approved', 'location=Nairobi', 'level=b-lt'],
      out: 'signed.pdf',
    },
    { label: 'Renew a timestamp', form: ['files=@signed.pdf', 'signature=renew'], out: 'renewed.pdf' },
  ],
  fields: [{ form: ['files=@form.pdf'] }],
  fill: [
    {
      form: ['files=@form.pdf', 'data={"name":"Ada","date":"2026-11-06","agree":true}'],
      out: 'filled.pdf',
    },
  ],
};

const quote = (word) => {
  if (/^[\w@.=/:,+-]+$/.test(word)) return word;
  if (word.includes("'")) throw new Error(`cannot quote ${word}`);
  return word.includes('$') ? `"${word}"` : `'${word}'`;
};

// A JSON body in single quotes: on one line when short, else a key a line.
function body(json) {
  const short = JSON.stringify(json);
  if (short.length <= 56) return `'${short}'`;
  const keys = Object.entries(json).map(([key, value]) => `    ${JSON.stringify(key)}: ${JSON.stringify(value)}`);
  return `'{\n${keys.join(',\n')}\n  }'`;
}

// curl's arguments, kept whole so a flag never ends a line without its value.
function curl(method, path, operation, sample) {
  const args = [];
  if (sample.method) args.push(`-X ${sample.method}`);
  if (operation.security?.length) args.push(`-H "X-Api-Key: ${sample.key ?? '$KEY'}"`);
  for (const header of sample.headers ?? []) args.push(`-H "${header}"`);
  if (sample.json) {
    args.push('-H "Content-Type: application/json"', `-d ${typeof sample.json === 'string' ? sample.json : body(sample.json)}`);
  }
  for (const field of sample.form ?? []) args.push(`-F ${quote(field)}`);
  const params = { ...PARAMS, ...sample.params };
  const url = BASE + path.replace(/\{(\w+)\}/g, (_, name) => params[name]) + (sample.query ? `?${sample.query}` : '');
  args.push((/[$?&]/.test(url) ? `"${url}"` : url) + (sample.out ? ` -o ${sample.out}` : ''));

  const lines = ['curl'];
  for (const arg of args) {
    const last = lines.length - 1;
    const width = lines[last].split('\n').pop().length + 1 + arg.split('\n')[0].length;
    if (width > 76) lines.push(`  ${arg}`);
    else lines[last] += ` ${arg}`;
  }
  return lines.join(' \\\n');
}

// What an example sends and gets back, for the languages other than curl.
function describe(method, path, operation, sample) {
  const params = { ...PARAMS, ...sample.params };
  const headers = operation.security?.length ? [['X-Api-Key', sample.key ?? '$KEY']] : [];
  for (const header of sample.headers ?? []) {
    const colon = header.indexOf(':');
    headers.push([header.slice(0, colon), header.slice(colon + 1).trim()]);
  }
  const form = sample.form?.map((field) => {
    const name = field.slice(0, field.indexOf('='));
    const value = field.slice(name.length + 1);
    if (value[0] === '@') return { name, file: value.slice(1) };
    return value[0] === '<' ? { name, text: value.slice(1) } : { name, value };
  });
  const replies = Object.entries(operation.responses)
    .filter(([status]) => status[0] === '2')
    .map(([, response]) => Object.keys(response.content ?? {}));
  const kind = replies.some((types) => types.includes('application/json')) ? 'json' : replies.some((types) => types.length) ? 'text' : 'none';
  return {
    method: method.toUpperCase(),
    url: BASE + path.replace(/\{(\w+)\}/g, (_, name) => params[name]) + (sample.query ? `?${sample.query}` : ''),
    headers,
    form,
    json: typeof sample.json === 'object' ? sample.json : undefined,
    jsonFile: typeof sample.json === 'string' ? sample.json.slice(1) : undefined,
    out: sample.out,
    reply: sample.out ? 'file' : kind,
  };
}

function check(id, operation, sample, schemas) {
  const fail = (what) => {
    throw new Error(`API example for ${id}: ${what}`);
  };
  const properties = (type) => {
    const schema = operation.requestBody?.content?.[type]?.schema;
    if (!schema) fail(`the operation takes no ${type} body`);
    return schemas[schema.$ref.split('/').pop()];
  };
  if (sample.form) {
    const schema = properties('multipart/form-data');
    const sent = sample.form.map((field) => field.slice(0, field.indexOf('=')));
    for (const name of sent) if (!schema.properties[name]) fail(`'${name}' is not a field of the form`);
    for (const name of schema.required ?? []) if (!sent.includes(name)) fail(`the form needs '${name}'`);
  }
  if (sample.json && typeof sample.json !== 'string') {
    const schema = properties('application/json');
    for (const name of Object.keys(sample.json)) if (!schema.properties[name]) fail(`'${name}' is not in the JSON body`);
    for (const name of schema.required ?? []) if (!(name in sample.json)) fail(`the JSON body needs '${name}'`);
  }
  const known = (operation.parameters ?? []).filter((p) => p.in === 'header').map((p) => p.name.toLowerCase());
  for (const header of sample.headers ?? []) {
    const name = header.slice(0, header.indexOf(':')).toLowerCase();
    if (!known.includes(name)) fail(`'${name}' is not a header the operation documents`);
  }
}

/** Writes the prepared description and returns its path, relative to the site. */
// The service's tags are identifiers; the sidebar and the tag pages show these.
// Each keeps the tag's slug, so the pages' addresses do not change.
const TAGS = {
  render: 'Render',
  convert: 'Convert',
  pdf: 'PDF',
  'pdf-forms': 'PDF forms',
  templates: 'Templates',
  jobs: 'Jobs',
  fonts: 'Fonts',
  service: 'Service',
};
const tagName = (tag) => {
  if (!TAGS[tag]) throw new Error(`API tag ${tag} has no name; add it to scripts/api-spec.mjs`);
  return TAGS[tag];
};

export function prepareApiSpec() {
  const spec = JSON.parse(readFileSync(join(root, 'public/openapi.json'), 'utf8'));
  spec.servers = [{ url: BASE }];
  spec.info.description += OVERVIEW;
  for (const tag of spec.tags) tag.name = tagName(tag.name);

  const seen = new Set();
  for (const [path, methods] of Object.entries(spec.paths)) {
    for (const [method, operation] of Object.entries(methods)) {
      const id = operation.operationId;
      const samples = SAMPLES[id];
      if (!samples) throw new Error(`API example missing for ${id} (${method.toUpperCase()} ${path}); add it to scripts/api-spec.mjs`);
      seen.add(id);
      operation.tags = operation.tags.map(tagName);
      // Most callers send a form, so show it before the JSON body.
      const content = operation.requestBody?.content;
      if (content?.['multipart/form-data'] && Object.keys(content)[0] !== 'multipart/form-data') {
        const { 'multipart/form-data': form, ...others } = content;
        operation.requestBody.content = { 'multipart/form-data': form, ...others };
      }
      // Every example in every language, curl first. The page's picker reads
      // 'Python: Stored template' as a language and the example's name.
      operation['x-codeSamples'] = samples.flatMap((sample) => {
        check(id, operation, sample, spec.components.schemas);
        const request = describe(method, path, operation, sample);
        return [
          { lang: 'bash', name: 'curl', source: curl(method, path, operation, sample) },
          ...clients.map(({ lang, name, write }) => ({ lang, name, source: write(request) })),
        ].map(({ lang, name, source }) => ({ lang, label: sample.label ? `${name}: ${sample.label}` : name, source }));
      });
    }
  }
  const stray = Object.keys(SAMPLES).filter((id) => !seen.has(id));
  if (stray.length) throw new Error(`API examples for operations that no longer exist: ${stray.join(', ')}`);

  const out = join(root, '.cache/openapi.json');
  mkdirSync(dirname(out), { recursive: true });
  writeFileSync(out, JSON.stringify(spec, null, 2));
  return './.cache/openapi.json';
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const spec = JSON.parse(readFileSync(join(root, prepareApiSpec()), 'utf8'));
  for (const methods of Object.values(spec.paths))
    for (const operation of Object.values(methods))
      for (const sample of operation['x-codeSamples']) console.log(`# ${operation.operationId}: ${sample.label}\n${sample.source}\n`);
}

// Checks the framing of streamed answers against the worst chunk boundaries a
// network can produce. Run with: node sdk/javascript/test/frameJson.test.mjs
//
// frameJson is pulled out of the source rather than reimplemented, so this tests
// what actually ships.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import assert from 'node:assert/strict';

const here = dirname(fileURLToPath(import.meta.url));
const artifact = 'BraidworkKikoSDK.min.js';
const source = readFileSync(join(here, '..', artifact), 'utf8');

/** The fn named `frameJson`, sliced out of `source` down to its closing brace. */
function extractFrameJson(source) {
    const start = source.indexOf('function frameJson(');
    assert.ok(start > -1, `frameJson not found in ${artifact}`);
    // Counting braces is safe only outside strings, and this body is full of
    // quoted braces (`'{'`, `'}'`) that a naive count would walk straight into.
    let depth = 0;
    let quote = null;
    let escaped = false;
    for (let i = source.indexOf('{', start); i < source.length; i++) {
        const c = source[i];
        if (quote) {
            if (escaped) escaped = false;
            else if (c === '\\') escaped = true;
            else if (c === quote) quote = null;
            continue;
        }
        if (c === '"' || c === "'" || c === '`') quote = c;
        else if (c === '{') depth += 1;
        else if (c === '}' && --depth === 0) return source.slice(start, i + 1);
    }
    assert.fail(`frameJson has no closing brace in ${artifact}`);
}

const frameJson = eval(`(${extractFrameJson(source)})`);

// Quotes, braces and a backslash inside the text: none of them may be mistaken
// for framing.
const DELTAS = [
    { contentSeq: 0, content: 'Hello, ' },
    { contentSeq: 0, content: '世' },
    { contentSeq: 0, content: '界 " {quoted} \\ done' },
    { contentSeq: 1, content: 'Second' },
];

const ENVELOPE = {
    status: 200,
    data: {
        sessionId: 's1',
        answers: [],
        collectData: [{ k: 'v' }],
        nextAction: 'Terminate',
    },
    err: null,
};

/** The bytes a real answer produces: one JSON document per line. */
function body() {
    const frames = DELTAS.concat([{ contentSeq: null, content: JSON.stringify(ENVELOPE) }]);
    return new TextEncoder().encode(frames.map((f) => JSON.stringify(f) + '\n').join(''));
}

/** Feeds bytes through the same decoder loop the SDK uses. */
function collect(bytes, chunkSize) {
    const decoder = new TextDecoder('utf-8');
    const state = { carry: '' };
    const frames = [];
    for (let i = 0; i < bytes.length; i += chunkSize) {
        const text = decoder.decode(bytes.slice(i, i + chunkSize), { stream: true });
        frames.push(...frameJson(state, text));
    }
    frames.push(...frameJson(state, decoder.decode()));
    return frames;
}

function check(frames, where) {
    assert.equal(frames.length, 5, `${where}: got ${frames.length} frames`);
    assert.deepEqual(frames.slice(0, 4), DELTAS.map((f) => JSON.stringify(f)), where);
    const terminal = JSON.parse(frames[4]);
    assert.equal(terminal.contentSeq, null, where);
    assert.deepEqual(JSON.parse(terminal.content), ENVELOPE, where);
}

const bytes = body();

// Everything at once, and every chunk size down to a single byte — which splits
// the multi-byte characters as badly as it is possible to split them.
for (const size of [bytes.length, 7, 3, 2, 1]) {
    check(collect(bytes, size), `chunk size ${size}`);
}

// Every possible single split point, including inside a frame and inside a
// multi-byte character.
for (let cut = 0; cut <= bytes.length; cut++) {
    const decoder = new TextDecoder('utf-8');
    const state = { carry: '' };
    const frames = [
        ...frameJson(state, decoder.decode(bytes.slice(0, cut), { stream: true })),
        ...frameJson(state, decoder.decode(bytes.slice(cut), { stream: true })),
        ...frameJson(state, decoder.decode()),
    ];
    check(frames, `cut at ${cut}`);
}

// A document that shares a chunk with others, and one whose text contains a
// newline: the framing is by JSON structure, not by scanning for newlines.
{
    const state = { carry: '' };
    const frames = frameJson(state, '{"contentSeq":0,"content":"a"}\n{"contentSeq":0,"content":"b\\nc"}\n');
    assert.equal(frames.length, 2);
    assert.equal(JSON.parse(frames[1]).content, 'b\nc');
    assert.equal(state.carry, '');
}

// An incomplete document is held, not emitted, and is completed by the next call.
{
    const state = { carry: '' };
    assert.deepEqual(frameJson(state, '{"contentSeq":0,"con'), []);
    assert.equal(state.carry, '{"contentSeq":0,"con');
    assert.deepEqual(frameJson(state, 'tent":"x"}\n'), ['{"contentSeq":0,"content":"x"}']);
    assert.equal(state.carry, '');
}

console.log('frameJson: all checks passed');

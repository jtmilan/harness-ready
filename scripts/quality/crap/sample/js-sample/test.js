// Only covers simpleAdd — gnarly is left uncovered on purpose.
const assert = require('assert');
const { simpleAdd } = require('./index.js');

assert.strictEqual(simpleAdd(2, 3), 5);
assert.strictEqual(simpleAdd(-1, 1), 0);
console.log('js-sample tests passed');

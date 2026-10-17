// Unit tests for the bar widget's model, plus a round trip through a real
// `omalive --demo` instance when OMALIVE_BIN is set.
const assert = require('node:assert/strict');
const model = require('../Model.js');

// Status lines fill missing fields and theme slots.
const status = model.decode('{"type":"status","voice":"live","listeners":2,"theme":{"red":"#ff0000"}}');
assert.equal(status.voice, 'live');
assert.equal(status.camera, 'off');
assert.equal(status.theme.red, '#ff0000');
assert.equal(status.theme.cyan, model.emptyStatus().theme.cyan);
assert.deepEqual(model.decode('{"type":"offline"}'), {type: 'offline'});
assert.throws(() => model.decode('garbage'));
assert.throws(() => model.decode('{"voice":"live"}'));

// Levels map -60…0 dBFS to 0…1, and junk to silence.
assert.equal(model.levelHeight(0), 1);
assert.equal(model.levelHeight(-60), 0);
assert.equal(model.levelHeight(-90), 0);
assert.equal(model.levelHeight(-30), 0.5);
assert.equal(model.levelHeight(null), 0);

// History keeps the newest values.
let history = [];
for (let i = 0; i < 20; i++) history = model.push(history, i, model.WAVE_COLUMNS);
assert.equal(history.length, model.WAVE_COLUMNS);
assert.equal(history[history.length - 1], 19);

// The waveform is mirrored around the centre row, which lights first.
const centre = (model.ROWS - 1) / 2;
assert.equal(model.waveLit(0, centre), false);
assert.equal(model.waveLit(0.1, centre), true);
assert.equal(model.waveLit(0.1, 0), false);
assert.equal(model.waveLit(1, 0), true);
for (let row = 0; row < model.ROWS; row++) {
  assert.equal(model.waveLit(0.5, row), model.waveLit(0.5, model.ROWS - 1 - row));
}

// Picture messages become fresh Image sources; malformed ones none.
assert.equal(model.pictureSource({path: '/run/user/1000/omalive.ppm', sequence: 7}), 'file:///run/user/1000/omalive.ppm?7');
assert.equal(model.pictureSource({}), '');

assert.equal(model.gradient(['#000000', '#ffffff'], 0.5), '#808080');
assert.equal(model.gradient(['#000000', '#ff0000', '#00ff00'], 1), '#00ff00');

assert.equal(model.micOnAir({voice: 'live', listeners: 1}), true);
assert.equal(model.micOnAir({voice: 'live', listeners: 0}), false);
assert.equal(model.micText({voice: 'live', listeners: 1}), 'Mic on air · 1 app listening');
assert.match(model.cameraText({camera: 'live', format: '1280×720 · 30 fps', look: 'Warm film'}), /Warm film/);
assert.match(model.tooltip(model.emptyStatus(), false), /not running/);

// Live round trip against a demo instance on a private socket.
if (process.env.OMALIVE_BIN) {
  const {spawn, spawnSync} = require('node:child_process');
  const {mkdtempSync} = require('node:fs');
  const socket = mkdtempSync('/tmp/omalive-test-') + '/omalive.sock';
  const env = {...process.env, OMALIVE_SOCKET: socket};
  const app = spawn(process.env.OMALIVE_BIN, ['--demo', '--background'], {env, stdio: 'ignore'});
  const bridge = spawn(process.env.OMALIVE_BIN, ['status', '--follow'], {env});
  const seen = {status: 0, level: 0, picture: 0, offline: 0};
  let last = null;
  let buffer = '';
  bridge.stdout.on('data', chunk => {
    buffer += chunk;
    let newline;
    while ((newline = buffer.indexOf('\n')) >= 0) {
      const message = model.decode(buffer.slice(0, newline));
      buffer = buffer.slice(newline + 1);
      seen[message.type]++;
      if (message.type === 'status') { last = message; begin(); }
      if (message.type === 'picture') {
        const ppm = require('node:fs').readFileSync(message.path);
        assert.equal(ppm.subarray(0, 2).toString(), 'P6');
      }
    }
  });
  // Timed from the first status, since the app takes a moment to start.
  let started = false;
  const begin = () => {
    if (started) return;
    started = true;
    // Watch the picture for a second and a half, as an open panel would.
    setTimeout(() => bridge.stdin.write('{"cmd":"picture","on":true}\n'), 500);
    setTimeout(() => bridge.stdin.write('{"cmd":"picture","on":false}\n'), 2000);
    setTimeout(() => bridge.stdin.write('{"cmd":"toggle-mute"}\n'), 2500);
    setTimeout(() => {
      bridge.stdin.write('{"cmd":"quit"}\n');
      bridge.stdin.end();
    }, 3500);
    setTimeout(() => {
      bridge.kill();
      app.kill();
      spawnSync('rm', ['-rf', require('node:path').dirname(socket)]);
      assert.ok(seen.status >= 2, 'status, then muted: ' + JSON.stringify(seen));
      assert.equal(last.voice, 'muted');
      assert.ok(seen.level > 10, 'levels while on air: ' + JSON.stringify(seen));
      assert.ok(seen.picture > 20 && seen.picture < 60, 'about 30 pictures a second, only while watched: ' + JSON.stringify(seen));
      console.log('PASS: model and live demo round trip', JSON.stringify(seen));
    }, 5500);
  };
  setTimeout(() => { if (!started) { app.kill(); bridge.kill(); throw new Error('omalive never reported status'); } }, 20000).unref();
} else {
  console.log('PASS: model');
}

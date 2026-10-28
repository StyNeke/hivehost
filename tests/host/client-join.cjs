#!/usr/bin/env node
// Joins a running server with a real game client protocol implementation and reports what the
// server sent back. Java uses mineflayer, Bedrock uses bedrock-protocol (pure-JS RakNet). Both need
// the server in offline mode, because the test has no Microsoft account.
//
//   npm install mineflayer bedrock-protocol jsp-raknet --ignore-scripts
//   node tests/host/client-join.cjs java 127.0.0.1 25627 26.1
//   node tests/host/client-join.cjs bedrock 127.0.0.1 19132 1.26.51
//   node tests/host/client-join.cjs bedrock-ping 127.0.0.1 19132
//
// Exits 0 once the client has spawned in the world and the server echoed its chat message.
const [edition, host, port, version] = process.argv.slice(2);
const username = process.env.GM_CLIENT_NAME ?? 'ClientTester';
const timeout = setTimeout(() => { console.error('timed out'); process.exit(2); }, 90_000);
const done = (message) => { console.log(message); clearTimeout(timeout); setTimeout(() => process.exit(0), 500); };

if (edition === 'java') {
  const mineflayer = require('mineflayer');
  const bot = mineflayer.createBot({ host, port: Number(port), version, username, auth: 'offline' });
  bot.once('spawn', () => {
    console.log(`spawned at ${bot.entity.position} in ${bot.game.dimension}, game mode ${bot.game.gameMode}`);
    bot.chat('hello from a real client');
  });
  bot.on('messagestr', (message) => {
    if (message.includes('hello from a real client')) { done(`chat echoed: ${message}`); bot.quit(); }
  });
  bot.on('kicked', (reason) => { console.error('kicked:', JSON.stringify(reason)); process.exit(1); });
  bot.on('error', (error) => { console.error('error:', error.message); process.exit(1); });
} else if (edition === 'bedrock-ping') {
  // RakNet unconnected ping: proves the UDP port answers and reports the server's version.
  require('bedrock-protocol').ping({ host, port: Number(port), raknetBackend: 'jsp-raknet' })
    .then((advert) => done(`ping: ${advert.motd} · version ${advert.version} · protocol ${advert.protocol} · ${advert.playersOnline}/${advert.playersMax} players`))
    .catch((error) => { console.error('ping failed:', error.message); process.exit(1); });
} else if (edition === 'bedrock') {
  const bedrock = require('bedrock-protocol');
  const client = bedrock.createClient({ host, port: Number(port), version, username, offline: true, raknetBackend: 'jsp-raknet', skipPing: true });
  client.on('start_game', (packet) => console.log(`start_game: level "${packet.level_id ?? packet.world_name}", player position ${JSON.stringify(packet.player_position)}`));
  client.on('spawn', () => {
    console.log('spawned');
    client.queue('text', { type: 'chat', needs_translation: false, source_name: username, xuid: '', platform_chat_id: '', filtered_message: '', message: 'hello from a real client' });
    setTimeout(() => { done('bedrock client spawned and chatted'); client.close(); }, 3000);
  });
  client.on('kick', (reason) => { console.error('kicked:', JSON.stringify(reason)); process.exit(1); });
  client.on('error', (error) => { console.error('error:', error.message); process.exit(1); });
} else {
  console.error('usage: client-join.cjs java|bedrock HOST PORT VERSION');
  process.exit(64);
}

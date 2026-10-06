import { chromium } from 'playwright';
const baseUrl = process.env.DJ_STORY_TEST_URL ?? 'http://127.0.0.1:17703';
const browser = await chromium.launch({ channel: 'chrome', headless: true });
const page = await browser.newPage();
const pageErrors = [];
page.on('pageerror', error => pageErrors.push(error.message));
try {
 await page.route(new URL('/story-test', baseUrl).href, route => route.fulfill({ contentType: 'text/html', body: '<html><body><div id="story"></div></body></html>' }));
 await page.route(url => url.pathname.startsWith('/api/'), route => route.fulfill({ contentType: 'application/json', body: '{"features":null}' }));
 // Count real component calls without adding production instrumentation.
 await page.route(url => url.pathname.endsWith('/dj-cockpit/transition_scene.ts'), async route => {
  const response = await route.fetch();
  const body = (await response.text()).replace(/function sceneMarkers\([^)]*\)\s*\{/,
   match => `${match}\n globalThis.__sceneMarkerCalls = (globalThis.__sceneMarkerCalls ?? 0) + 1;`);
  await route.fulfill({ response, body });
 });
 await page.goto(new URL('/story-test', baseUrl).href);
 const results = await page.evaluate(async () => {
  const { default: Harness, mount, tick, unmount, writable } = await import('/src/lib/components/dj-cockpit/fixtures/TransitionStoryHarness.svelte');
  const player = await import('/src/lib/stores/player.ts');
  const deck = id => ({media_ref_kind:'library_track',media_ref_id:String(id),title:'Track '+id,
   beat_markers_ms:[], downbeat_markers_ms:[],phrase_markers_ms:[],drop_markers_ms:[],manual_drop_markers_ms:[],
   profile_ready:true,profile_status:'ready',waveform_status:'ready',waveform_peaks:[],safe_crossfade_only:false});
  const program = {template:'SafeCrossfade',tier:'SafeCrossfade',sample_rate:1000,channels:2,
   deck_a_start_frame:0,deck_b_start_frame:0,sync_start:0,intro_start:0,swap_start:2000,fade_start:2000,resolve_at:4000,loops:[],
   automation:[{param:{DeckGain:'A'},start_sample:0,end_sample:4000,from:1,to:0,curve:'EqualPowerOut'},
   {param:{DeckGain:'B'},start_sample:0,end_sample:4000,from:0,to:1,curve:'EqualPowerIn'}]};
  const armed = {enabled:true,current:deck(1),next:deck(2),planning_status:'armed',timing_status:'armed',
   transition_plan:program,last_transition_event_id:7,playback_position_ms:70000,planned_start_ms:100000,
   drop_preview:{status:'armed',planned_fire_ms:90000,incoming_drop_ms:45000}};
  player.currentTrack.set({id:1,tidal_id:null,title:'Track 1'});player.position.set(70000);player.isPlaying.set(true);player.playbackSeekRevision.set(0);
  const statuses=writable(armed);
  const component=mount(Harness,{target:document.querySelector('#story'),props:{statuses}});
  await tick();
  const previewBeforeSeek=document.querySelector('.scene-heading')?.textContent;
  player.position.set(85000);player.playbackSeekRevision.update(v=>v+1);await tick();
  const didNotFireAfterSeek=!document.querySelector('.cursor');
  statuses.set({...armed,playback_position_ms:91000,
   drop_preview:{status:'fired',actual_fire_ms:90005,incoming_drop_ms:45000}});
  player.position.set(91000);await tick();
  const previewConfirmedWithoutFakeMix=document.querySelector('.transition-story')?.textContent?.includes('Drop preview fired')
   && !document.querySelector('.cursor');
  const fired = {...armed,current:deck(2),next:deck(3),playback_position_ms:1500,
   active_transition:{event_id:7,outgoing:deck(1),incoming:deck(2),program,start_ms:100000,actual_start_ms:100007,elapsed_ms:1500}};
  statuses.set(fired);player.currentTrack.set({id:2,tidal_id:null,title:'Track 2'});player.position.set(1500);await tick();
  const firesAfterArmedSeek=Boolean(document.querySelector('.cursor'));
  const hasMixingProgress=document.querySelector('.live-state')?.textContent?.includes('Mixing now') && Number(document.querySelector('progress')?.value)>0;
  player.position.set(1700);player.playbackSeekRevision.update(v=>v+1);await tick();
  const cancelsLiveSeek=!document.querySelector('.cursor');
  const rhythmicProgram={...program,template:'ClubMix',tier:'FullBlend',resolve_at:10000};
  const grid={...deck(2),beat_confidence:0.9,analysis_scope_ms:200000,
   beat_markers_ms:Array.from({length:200},(_,i)=>i*1000)};
  statuses.set({...fired,transition_plan:rhythmicProgram,
   active_transition:{...fired.active_transition,event_id:8,program:rhythmicProgram,outgoing:{...grid,...deck(1)},incoming:grid}});
  player.position.set(1500);await tick();
  const markerCallsBefore=globalThis.__sceneMarkerCalls;
  for (const ms of [1550,1600,1650,1700]) {player.position.set(ms);await tick();}
  const markerCallsAfter=globalThis.__sceneMarkerCalls;
  const markerGeometryCached=markerCallsBefore>=2 && markerCallsBefore===markerCallsAfter;
  player.isPlaying.set(false);await unmount(component);
  return {previewBeforeSeek,didNotFireAfterSeek,previewConfirmedWithoutFakeMix,firesAfterArmedSeek,hasMixingProgress,cancelsLiveSeek,
   markerGeometryCached,markerCallsBefore,markerCallsAfter};
 });
 console.log(JSON.stringify({ ...results, pageErrors }));
 if (!results.didNotFireAfterSeek || !results.previewConfirmedWithoutFakeMix || !results.firesAfterArmedSeek || !results.hasMixingProgress || !results.cancelsLiveSeek || !results.markerGeometryCached || pageErrors.length) process.exitCode=1;
} finally { await browser.close(); }

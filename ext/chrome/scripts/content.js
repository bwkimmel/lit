var bookId = null;
var video = null;
var cues = null;
var updateId = null;
var selectedWord = null;

function removeCueContainer() {
  var ctr = document.getElementById('lit-cue-container');
  if (ctr) {
    ctr.remove();
  }
  cues = null;
}

function stopUpdating() {
  if (updateId == null) {
    return;
  }
  clearInterval(updateId);
  updateId = null;
}

async function onSeeked() {
  cues = null;
  await update();
}

async function onEnded() {
  stopUpdating();
  removeCueContainer();
}

async function onPaused() {
  stopUpdating();
  await update();
}

async function onPlay() {
  if (updateId == null) {
    updateId = setInterval(update, 50);
  }
  const ctr = document.getElementById('lit-panel-container');
  if (ctr != null) {
    ctr.classList.remove('lit-show');
  }
}

async function onWordClick(e) {
  pauseAfterCueHovering = true;
  if (selectedWord != null) {
    selectedWord.classList.remove('lit-selected');
  }
  selectedWord = e.target;
  selectedWord.classList.add('lit-selected');
  const offset = Number(e.target.getAttribute('data-lit-word-offset'));
  const url = `http://localhost:5080/api/books/${bookId}/words/${offset}`;
  const resp = await fetch(url);
  if (!resp.ok) {
    console.warn(`Could not fetch word info at offset ${offset}: ${await resp.body()}`);
    return;
  }
  const json = await resp.json();
  var words = Array();
  if (json.deps) {
    for (const dep of json.deps) {
      if (words.includes(dep.text)) {
        continue;
      }
      words.push(dep.text);
    }
  }
  if (!words.includes(json.text)) {
    words.push(json.text);
  }
  const list = words.join(',');
  const defineURL = `http://localhost:5080/define/${list}/edit`;
  window.open(defineURL, 'lit-define');
  document.getElementById('lit-panel-container').classList.add('lit-show');
}

var pauseAfterCueHovering = false;
var cueHovering = false;
var cueHoveringTimeout = null;

async function onCueMouseOver(e) {
  if (cueHoveringTimeout) {
    clearTimeout(cueHoveringTimeout);
    cueHoveringTimeout = null;
  }
  if (cueHovering) {
    return;
  }
  cueHovering = true;
  pauseAfterCueHovering = video.paused;
  video.pause();
}

async function onCueMouseOut(e) {
  if (cueHoveringTimeout) {
    clearTimeout(cueHoveringTimeout);
  }
  cueHoveringTimeout = setTimeout(function() {
    cueHovering = false;
    if (!pauseAfterCueHovering) {
      video.play();
    }
  }, 250);
}

function htmlDecode(input) {
  var textarea = document.createElement('textarea');
  textarea.innerHTML = input;
  return textarea.value;
}

async function update() {
  if (!video) { return; }
  const t = video.currentTime;

  var needFetch = false;
  var dirty = false;

  if (cues == null) {
    needFetch = true;
    dirty = true;
  }

  while (cues && cues.length > 1 && cues[1].end <= t) {
    dirty = true;
    cues.shift();
  }

  if (cues && cues.length > 0 && cues[cues.length - 1].start <= t) {
    needFetch = true;
    dirty = true;
  }

  if (!dirty) {
    return;
  }

  if (needFetch) {
    await fetchCues(t);
    while (cues.length > 1 && cues[1].end <= t) {
      cues.shift();
    }
  }

  const container = document.createElement('div');
  container.setAttribute('id', 'lit-cue-container');

  for (const cue of cues) {
    if (cue.lines == null) {
      continue;
    }
    const div = document.createElement('div');
    div.classList.add('lit-vtt-cue');
    if (t < cue.start) {
      div.classList.add('lit-next-cue');
    } else if (t < cue.end) {
      div.classList.add('lit-active-cue');
    } else {
      div.classList.add('lit-prev-cue');
    }
    div.addEventListener('mouseover', onCueMouseOver);
    div.addEventListener('mouseout', onCueMouseOut);
    
    for (let i = 0; i < cue.lines.length; i++) {
      const line = cue.lines[i];
      if (i > 0) {
        const br = document.createElement('br');
        div.appendChild(br);
      }
      for (const token of line.tokens) {
        const text = document.createTextNode(htmlDecode(token.text));
        var node = text;
        if (token.word) {
          const span = document.createElement('span');
          span.appendChild(text);
          span.classList.add('lit-word');
          if (token.word.min_status < token.word.max_status) {
            span.classList.add(`lit-min-status-${token.word.min_status}`);
          }
          span.classList.add(`lit-max-status-${token.word.max_status}`);
          span.setAttribute('data-lit-word-offset', `${token.word.offset}`);
          span.addEventListener('click', onWordClick);
          node = span;
        }
        div.appendChild(node);
      }
    }
    container.appendChild(div);
  }

  var old = document.getElementById('lit-cue-container');
  if (old) {
    old.remove();
  }

  // FIXME: this works for youtube, but is not generic. We'll have to use
  // javascript to overlay the cue container on top of the video, rather than
  // being able to use CSS for this.
  video.parentElement.parentElement.appendChild(container);

  tippy('.lit-word', {
    content: 'Loading...',
    allowHTML: true,
    onTrigger(instance, event) {
      const offset = Number(event.target.getAttribute('data-lit-word-offset'));
      fetch(`http://localhost:5080/api/books/${bookId}/words/${offset}`)
        .then((resp) => resp.json())
        .then((word) => {
          instance.setContent(generateTooltipContent(word));
        })
        .catch((error) => {
          console.warn("Failed to load tooltip");
          instance.setContent("ERROR: " + JSON.stringify(error, null, 2));
        });
    },
  });
}

function generateTooltipContent(word) {
  const div = document.createElement('lit-tooltiptext');
  div.appendChild(generateDefinitions(word.defs, word.deps));
  return div;
}

function generateDefinitions(defs, deps) {
  const ul = document.createElement('ul');
  ul.classList.add('lit-word-definitions');

  for (const def of defs) {
    const li = document.createElement('li');
    ul.appendChild(li);

    const divText = document.createElement('div');
    li.appendChild(divText);
    divText.classList.add('lit-word-text');
    if (def.resolved_status[0] < def.resolved_status[1]) {
      divText.classList.add(`lit-min-status-${def.resolved_status[0]}`);
    }
    divText.classList.add(`lit-max-status-${def.resolved_status[1]}`);
    if (def.inherit) {
      divText.classList.add('lit-inherit');
    }
    divText.innerText = def.text;

    if (def.pronunciation) {
      const divPro = document.createElement('div');
      li.appendChild(divPro);
      divPro.classList.add('lit-word-pronunciation');
      divPro.innerText = def.pronuncation;
    }

    const divTr = document.createElement('div');
    li.appendChild(divTr);
    divTr.classList.add('list-word-translation');
    divTr.innerHTML = def.translation_html;

    if (def.image_file) {
      const img = document.createElement('img');
      li.appendChild(img);
      img.classList.add('lit-word-image');
      img.setAttribute('alt', def.text);
      img.setAttribute('src', `http://localhost:5080/words/${def.id}/image?w=150&h=100`);
    }

    if (def.tags) {
      const ulTags = document.createElement('ul');
      li.appendChild(ulTags);
      ulTags.classList.add('lit-word-tags');
      for (const tag of def.tags) {
        const liTag = document.createElement('li');
        ulTags.appendChild(liTag);
        liTag.innerText = tag;
      }
    }

    if (def.parents) {
      const ulParents = document.createElement('ul');
      li.appendChild(ulParents);
      ulParents.classList.add('lit-word-parents');
      
      for (const parent of def.parents) {
        var parentDefs = Array();
        if (deps) {
          for (const dep of deps) {
            if (dep.text == parent) {
              parentDefs.push(dep);
            }
          }
        }
        const liParent = document.createElement('li');
        ulParents.appendChild(liParent);
        liParent.appendChild(generateDefinitions(parentDefs, deps));
      }
    }
  }

  return ul;
}

async function fetchCues(t) {
  const resp = await fetch(`http://localhost:5080/api/books/${bookId}/cues/${t}`);
  if (!resp.ok) {
    console.warn(`Could not fetch cues from book ${bookId} at time ${t}: ${await resp.text()}`);
    return;
  }
  const json = await resp.json();
  cues = json.cues;
  if (cues.length == 0 || cues[cues.length - 1].start <= t) {
    cues.push({
      start: video.duration + 1.0,
      end: video.duration + 2.0,
      lines: null
    });
  }
}

async function setupVideoPage(url) {
  const resp = await fetch(`http://localhost:5080/api/books?url=${encodeURIComponent(canonicalizeURL(url))}`);
  if (!resp.ok) {
    console.warn(`Could not query book for URL ${url}: ${await resp.body()}`);
    return false;
  }
  const json = await resp.json();
  if (json.length == 0) {
    console.log(`No LIT book for URL ${url}`);
    return false;
  }
  if (json.length > 1) {
    console.warn(`Multiple LIT books for URL ${url}: ${json} (using first one)`);
  }
  bookId = json[0].id;
  console.log(`LIT book: http://localhost:5080/read/${bookId}`);
  return true;
}

async function connectVideo() {
  video.addEventListener("seeked", onSeeked);
  video.addEventListener("paused", onPaused);
  video.addEventListener("ended", onEnded);
  video.addEventListener("play", onPlay);
  if (!video.paused) {
    await onPlay();
  }
}

async function initVideo() {
  const v = document.getElementsByTagName('video');
  if (v.length > 0) {
    video = v[0];
    await connectVideo();
    return;
  }
  console.log("No video element found, watching for video node...");
  const poll = setInterval(function() {
    const v = document.getElementsByTagName('video');
    if (v.length == 0) {
      return;
    }
    clearInterval(poll);
    console.log("Found video element.");
    video = v[0];
    connectVideo();
  }, 100);
}

async function init(url) {
  var e = document.getElementById('lit-panel-container');
  if (!await setupVideoPage(url)) {
    if (e) {
      e.remove();
    }
    return;
  }
  initVideo();
  if (e) {
    return;
  }
  e = document.createElement('div');
  e.setAttribute("id", "lit-panel-container");
  e.innerHTML = `
    <div class="lit-grid-container" style="grid-template-columns: 2fr 5px 1fr;">
      <div id="lit-panel-background"></div>
      <div id="lit-vertical-gutter" class="lit-gutter">
        <div id="lit-toggle-fullscreen" class="lit-gutter-button" onmousedown="event.stopPropagation(); event.preventDefault();">&#x25C0;</div>
        <div id="lit-close" class="lit-gutter-button" onmousedown="event.stopPropagation(); event.preventDefault();">&#x1F5D9;</div>
      </div>
      <div id="lit-panel">
        <iframe name="lit-define"></iframe>
      </div>
    </div>
  `;
  document.body.appendChild(e);
  const grid = e.querySelector('.lit-grid-container');
  const btn = e.querySelector('#lit-toggle-fullscreen');
  btn.addEventListener('click', function() {
    if (e.classList.contains('lit-fullscreen')) {
      grid.setAttribute('style', grid.getAttribute('data-lit-style'));
      grid.removeAttribute('data-lit-style');
      e.classList.remove('lit-fullscreen');
      btn.innerHTML = '&#x25C0;';
    } else {
      e.classList.add('lit-fullscreen');
      btn.innerHTML = '&#x25B6;';
      grid.setAttribute('data-lit-style', grid.getAttribute('style'));
      grid.removeAttribute('style');
    }
  });
  e.querySelector('#lit-close').addEventListener('click', function() {
    document.getElementById('lit-panel-container').classList.remove('lit-show');
  });

  Split({
    columnGutters: [{
      track: 1,
      element: document.getElementById('lit-vertical-gutter')
    }]
  });
}

init(document.URL);
window.navigation.addEventListener("navigate", (event) => {
  stopUpdating();
  removeCueContainer();
  bookId = null;
  video = null;
  init(event.destination.url);
});

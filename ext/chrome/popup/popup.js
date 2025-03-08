function canonicalizeURL(url) {
  const u = URL.parse(url);
  if (u == null) {
    console.warn(`Cannot canonicalize invalid URL: ${url}`);
    return url;
  }
  if (u.host == "www.youtube.com" && u.pathname == "/watch") {
    const videoId = u.searchParams.get('v');
    if (videoId == null) {
      console.warn(`Youtube watch URL has no video ID: ${url}`);
      return url;
    }
    return `https://www.youtube.com/watch?v=${videoId}`;
  }
  return url;
}

const tabs = await chrome.tabs.query({
  active: true,
  currentWindow: true
});

const tmpl = document.getElementById("book-template");
const ul = document.getElementById("book-list");

for (const tab of tabs) {
  const url = canonicalizeURL(tab.url);
  const resp = await fetch(`http://localhost:5080/api/books?url=${encodeURIComponent(url)}`);
  if (!resp.ok) {
    console.warn(`Failed query books for URL ${url}: ${await resp.text()}`);
    break;
  }
  const books = await resp.json();
  for (const book of books) {
    const li = tmpl.content.firstElementChild.cloneNode(true);
    const h3 = li.querySelector('.title');
    var slug = book.id.toString();
    if (book.hasOwnProperty('slug') && book.slug != "") {
      slug = book.slug;
    }
    h3.innerText = book.title;
    h3.addEventListener("click", function() {
      chrome.tabs.update({
        url: `http://localhost:5080/read/${slug}`
      });
    });
    if (book.hasOwnProperty('last_read')) {
      li.classList.add('read');
      li.querySelector('.last-read').innerText = book.last_read;
    } else {
      li.classList.add('unread');
    }
    li.querySelector('.button').addEventListener("click", async function() {
      {
        const resp = await fetch(`http://localhost:5080/api/books/${book.id}/read`, {
          method: 'POST',
        });
        if (!resp.ok) {
          console.warn(`Could not mark book ${book.id} as read: ${await resp.text()}`);
          return;
        }
      }
      {
        const resp = await fetch(`http://localhost:5080/api/books/${book.id}`);
        if (!resp.ok) {
          console.warn(`Could not fetch book ${book.id} after marking as read: ${await resp.text()}`);
          return;
        }
        const info = await resp.json();
        li.querySelector('.last-read').innerText = info.last_read;
        li.classList.remove('unread');
        li.classList.add('read');
        chrome.runtime.sendMessage({ tabId: tab.id, url: tab.url });
      }
    });
    ul.appendChild(li);
  }
}

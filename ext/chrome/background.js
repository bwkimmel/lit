importScripts('./shared.js');

chrome.contextMenus.onClicked.addListener(function(info) {
  switch (info.menuItemId) {
    case "link": {
      const url = canonicalizeURL(info.linkUrl);
      chrome.tabs.update({
        url: `http://localhost:5080/video?url=${encodeURIComponent(url)}`
      });
      break;
    }
    case "page": {
      const url = canonicalizeURL(info.frameUrl);
      chrome.tabs.update({
        url: `http://localhost:5080/video?url=${encodeURIComponent(url)}`
      });
      break;
    }
    default:
      console.warn(`Invalid menu item: ${info.menuItemId}`);
  }
});

chrome.runtime.onInstalled.addListener(function() {
  chrome.contextMenus.create({
    title: "Import link as/go to LIT book",
    contexts: ["link"],
    id: "link",
    targetUrlPatterns: ["https://www.youtube.com/watch*"]
  });
  chrome.contextMenus.create({
    title: "Import this page as/go to LIT book",
    contexts: ["page"],
    id: "page",
    documentUrlPatterns: ["https://www.youtube.com/watch*"]
  });
});

function updateBadge(tabId, url) {
  fetch(`http://localhost:5080/api/books?url=${encodeURIComponent(canonicalizeURL(url))}`)
    .then((resp) => resp.json())
    .then((books) => {
      if (books.length == 0) {
        chrome.action.setBadgeText({tabId: tabId, text: ''});
        return;
      }
      var unread = 0;
      for (const book of books) {
        if (!book.hasOwnProperty('last_read')) {
          unread++;
        }
      }
      if (unread == 0) {
        chrome.action.setBadgeText({tabId: tabId, text: '\u2713'});
        chrome.action.setBadgeTextColor({tabId: tabId, color: '#000'});
        chrome.action.setBadgeBackgroundColor({tabId: tabId, color: '#8e8'});
      } else {
        chrome.action.setBadgeText({tabId: tabId, text: unread.toString()});
        chrome.action.setBadgeTextColor({tabId: tabId, color: '#000'});
        chrome.action.setBadgeBackgroundColor({tabId: tabId, color: '#88f'});
      }
    })
    .catch((error) => {
      console.warn(`Failed to fetch books: ${error}`);
    })
}

chrome.tabs.onUpdated.addListener(function(tabId, changeInfo, tab) {
  updateBadge(tabId, tab.url);
});

chrome.runtime.onMessage.addListener(function(request, sender, sendResponse) {
  updateBadge(request.tabId, request.url);
});

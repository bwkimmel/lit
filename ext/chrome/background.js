importScripts('./shared.js');

chrome.tabs.onUpdated.addListener(function(tabId, changeInfo, tab) {
  fetch(`http://localhost:5080/api/books?url=${encodeURIComponent(canonicalizeURL(tab.url))}`)
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
});

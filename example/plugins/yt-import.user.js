// ==UserScript==
// @name         Youtube LIT Links
// @namespace    http://tampermonkey.net/
// @version      2025-02-09
// @description  Add LIT links to Youtube videos.
// @author       You
// @match        https://www.youtube.com/*
// @icon         data:image/gif;base64,R0lGODlhAQABAAAAACH5BAEKAAEALAAAAAABAAEAAAICTAEAOw==
// @grant        none
// ==/UserScript==

// This is a TamperMonkey (http://tampermonkey.net) script to add links to
// Youtube to import videos into LIT.
(function() {
    'use strict';

    function getQueryVariable(search, variable) {
        var query = search.substring(1);
        var vars = query.split('&');
        for (var i = 0; i < vars.length; i++) {
            var pair = vars[i].split('=');
            if (decodeURIComponent(pair[0]) == variable) {
                return decodeURIComponent(pair[1]);
            }
        }
        return null;
    }

    function createLITLink(videoID) {
        const canonicalURL = `https://www.youtube.com/watch?v=${videoID}`;
        const litURL = `http://localhost:5080/video?url=${canonicalURL}`;
        const litLink = document.createElement('a');
        litLink.setAttribute('href', litURL);
        litLink.innerText = '📖';
        return litLink;
    }

    function tryAddLITLink(a) {
        const href = a.getAttribute('href');
        if (href == null) {
            return;
        }
        const url = URL.parse(href, window.location.href);
        if (!a.getAttribute('href').startsWith('/watch')) {
            return;
        }
        if (url.pathname != '/watch') {
            return;
        }
        const videoID = getQueryVariable(url.search, 'v');
        if (videoID == null) {
            return;
        }
        const sib = a.nextSibling;
        if (sib != null && sib.tagName == 'A') {
            const canonicalURL = `https://www.youtube.com/watch?v=${videoID}`;
            const litURL = `http://localhost:5080/video?url=${canonicalURL}`;
            const sibHRef = sib.getAttribute('href');
            if (sibHRef == litURL) {
                return;
            }
            if (sibHRef.startsWith('http://localhost:5080/video?url=')) {
                sib.remove();
            }
        }
        const litLink = createLITLink(videoID);
        a.parentNode.insertBefore(litLink, a.nextSibling);
    }

    function addLITLinks() {
        for (var a of document.getElementsByTagName('a')) {
            tryAddLITLink(a);
        }
        setTimeout(addLITLinks, 100);
    }

    function tryAddTitleLink() {
        if (window.location.pathname != '/watch') {
            console.log('not watch url');
            return;
        }
        const videoID = getQueryVariable(window.location.search, 'v');
        if (videoID == null) {
            console.log('no video id');
            return;
        }
        for (var h1 of document.querySelectorAll('#title.ytd-watch-metadata > h1')) {
            const litLink = createLITLink(videoID);
            h1.appendChild(litLink);
            return;
        }
        setTimeout(tryAddTitleLink, 100);
    }

    setTimeout(addLITLinks, 100);
    setTimeout(tryAddTitleLink, 100);

    // Your code here...
})();

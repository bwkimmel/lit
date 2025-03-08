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

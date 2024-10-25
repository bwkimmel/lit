function toggleCollapsible(e) {
  while (e && !e.classList.contains('collapsible')) {
    e = e.parentNode;
  }
  if (!e) {
    console.log("ERROR: not collapsible");
    return;
  }
  if (e.classList.contains('active')) {
    e.classList.remove('active');
  } else {
    e.classList.add('active');
  }
}

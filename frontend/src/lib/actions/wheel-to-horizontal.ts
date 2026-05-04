export function wheelToHorizontal(node: HTMLElement) {
  const onWheel = (e: WheelEvent) => {
    if (Math.abs(e.deltaY) <= Math.abs(e.deltaX)) return
    // No horizontal overflow → let the page scroll vertically. Without this,
    // grid-mode containers (no x-overflow) silently swallow every wheel event
    // mouse-over them and the page appears frozen.
    if (node.scrollWidth <= node.clientWidth) return
    e.preventDefault()
    node.scrollLeft += e.deltaY
  }
  node.addEventListener('wheel', onWheel, { passive: false })
  return { destroy: () => node.removeEventListener('wheel', onWheel) }
}

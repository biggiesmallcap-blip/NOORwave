export function wheelToHorizontal(node: HTMLElement) {
  const onWheel = (e: WheelEvent) => {
    if (Math.abs(e.deltaY) <= Math.abs(e.deltaX)) return
    e.preventDefault()
    node.scrollLeft += e.deltaY
  }
  node.addEventListener('wheel', onWheel, { passive: false })
  return { destroy: () => node.removeEventListener('wheel', onWheel) }
}

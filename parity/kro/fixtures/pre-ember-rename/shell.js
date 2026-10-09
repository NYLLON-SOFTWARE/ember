// One delegated adapter survives Turbo visits; the conversation frame can stay permanent.
(() => {
  if (window.matchboxWorkspace) return
  window.matchboxWorkspace = true
  const mobile = matchMedia('(max-width: 900px)')
  let observer
  let returnFocus
  let unreadOnly = false
  let fieldPointer
  const sidebar = () => document.querySelector('.mb-sidebar')
  const rail = () => document.querySelector('.mb-rail')
  const isOpen = () => document.body.classList.contains('mb-sidebar-open')

  function replaceableField(target) {
    return document.body.classList.contains('mb-app') && target instanceof HTMLInputElement &&
      ['text', 'search', 'email', 'url', 'tel', 'password'].includes(target.type) &&
      !target.disabled && !target.readOnly && !target.hasAttribute('data-matchbox-preserve-selection')
  }

  // Select on entry, not on every click: a second click can still position the caret.
  document.addEventListener('focusin', event => {
    if (replaceableField(event.target)) event.target.select()
  })
  document.addEventListener('pointerdown', event => {
    fieldPointer = replaceableField(event.target) && document.activeElement !== event.target && event.button === 0
      ? { field: event.target, x: event.clientX, y: event.clientY }
      : undefined
  })
  document.addEventListener('pointercancel', () => { fieldPointer = undefined })

  function setDrawer(open, restoreFocus = false) {
    open = open && mobile.matches
    if (open && !isOpen()) returnFocus = document.activeElement
    document.body.classList.toggle('mb-sidebar-open', open)
    document.querySelectorAll('[data-matchbox-sidebar-toggle]').forEach(button => {
      button.setAttribute('aria-expanded', String(open))
      button.setAttribute('aria-label', open ? 'Close conversations' : 'Open conversations')
    })
    const backdrop = document.querySelector('.mb-sidebar-backdrop')
    if (backdrop) backdrop.hidden = !open
    for (const region of [sidebar(), rail()]) {
      if (region) region.inert = mobile.matches && !open
    }
    for (const region of document.querySelectorAll('.mb-main, .mb-header')) region.inert = open
    if (open) sidebar()?.querySelector('[data-matchbox-sidebar-close]')?.focus()
    else if (restoreFocus) {
      const target = returnFocus?.isConnected ? returnFocus : document.querySelector('[data-matchbox-sidebar-toggle]')
      target?.focus()
    }
  }

  function filterRooms() {
    let visible = 0
    let unread = 0
    const rows = document.querySelectorAll('[data-matchbox-room-row]')
    for (const row of rows) {
      if (row.matches('a[href]')) {
        const active = new URL(row.href).pathname === location.pathname.replace(/\/@\d+$/, '')
        if (active) row.setAttribute('aria-current', 'page')
        else row.removeAttribute('aria-current')
      }
      const isUnread = row.classList.contains('unread') || !!row.querySelector('.unread')
      if (isUnread) unread++
      const show = !unreadOnly || isUnread
      const container = row.closest('form') || row
      container.hidden = !show
      if (show) visible++
    }
    const empty = document.querySelector('[data-matchbox-room-empty]')
    if (empty) {
      empty.hidden = visible > 0
      const text = unreadOnly ? 'You’re all caught up. No unread conversations.' : 'No conversations found.'
      if (empty.textContent !== text) empty.textContent = text
    }
    document.querySelectorAll('[data-matchbox-unread-count]').forEach(badge => {
      badge.hidden = unread === 0
      badge.textContent = String(unread)
    })
    document.querySelectorAll('[data-matchbox-activity]').forEach(button => button.setAttribute('aria-pressed', String(unreadOnly)))
    for (const link of document.querySelectorAll('.mb-rail a[href]')) {
      const path = new URL(link.href).pathname
      const active = path === location.pathname || (link.hasAttribute('data-matchbox-home') && /^\/rooms\/\d+(?:\/)?$/.test(location.pathname))
      if (active && !unreadOnly) link.setAttribute('aria-current', 'page')
      else link.removeAttribute('aria-current')
    }
  }

  function connect() {
    observer?.disconnect()
    if (!document.body.classList.contains('mb-app')) return
    document.body.classList.add('mb-enhanced')
    setDrawer(false)
    filterRooms()
    if (sidebar()) {
      observer = new MutationObserver(filterRooms)
      observer.observe(sidebar(), { childList: true, subtree: true, attributes: true, attributeFilter: ['class'] })
    }
  }

  document.addEventListener('click', event => {
    // A pointer's default click can collapse the focus selection. Restore it only for
    // the first stationary click; dragging and subsequent clicks retain native selection.
    if (fieldPointer?.field === event.target && Math.hypot(event.clientX - fieldPointer.x, event.clientY - fieldPointer.y) < 6) {
      event.target.select()
    }
    fieldPointer = undefined
    const target = event.target.closest('button, a')
    if (!target) return
    if (target.matches('[data-matchbox-sidebar-toggle]')) setDrawer(!isOpen(), isOpen())
    else if (target.matches('[data-matchbox-sidebar-close]')) setDrawer(false, true)
    else if (target.matches('[data-matchbox-activity]')) {
      unreadOnly = !unreadOnly
      filterRooms()
      if (mobile.matches) setDrawer(true)
    } else if (target.closest('.mb-rail, .mb-sidebar') && target.matches('a[href]')) {
      unreadOnly = false
      filterRooms()
      // Frame-contained actions (new DM) stay in the drawer until a conversation is chosen.
      if (target.hasAttribute('data-turbo-frame') && mobile.matches) setDrawer(true)
      else if (!target.hasAttribute('data-turbo-frame')) setDrawer(false)
    }
  })
  document.addEventListener('keydown', event => {
    if (!isOpen() || event.target.closest('dialog[open]') || document.querySelector('.mb-appearance-toggle[aria-expanded="true"]')) return
    if (event.key === 'Escape') {
      event.preventDefault()
      setDrawer(false, true)
    } else if (event.key === 'Tab') {
      const items = [...document.querySelectorAll('.mb-rail a[href], .mb-rail button, .mb-sidebar a[href], .mb-sidebar button, .mb-sidebar input')]
        .filter(item => !item.disabled && item.getClientRects().length && getComputedStyle(item).visibility !== 'hidden')
      const current = items.indexOf(document.activeElement)
      if (!items.length) return
      const next = event.shiftKey ? (current <= 0 ? items.length - 1 : current - 1) : (current + 1) % items.length
      event.preventDefault()
      items[next].focus()
    }
  })
  window.addEventListener('matchbox:close-sidebar', () => setDrawer(false))
  document.addEventListener('turbo:before-stream-render', event => {
    if (event.target.getAttribute('action') === 'matchbox_room_order_changed') {
      event.detail.render = () => window.dispatchEvent(new Event('matchbox:room-order-changed'))
    }
  })
  document.addEventListener('turbo:before-cache', () => { setDrawer(false); observer?.disconnect() })
  document.addEventListener('turbo:before-render', () => observer?.disconnect())
  document.addEventListener('turbo:render', connect)
  document.addEventListener('turbo:load', connect)
  mobile.addEventListener('change', () => setDrawer(false))
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', connect, { once: true })
  else connect()
})()

// One adapter survives Turbo visits. Only explicit-save channel forms opt in.
(() => {
  if (window.emberSettings) return
  window.emberSettings = true
  let state
  let continuation
  let switchDraft
  let bypass = false
  let saving = false
  let currentIndex
  let returningFromHistory
  const formSelector = 'form[data-ember-settings]'
  const dialog = () => document.querySelector('[data-ember-unsaved-dialog]')
  const fields = form => [...form.elements].filter(field => field.name && !field.disabled && !['submit', 'button'].includes(field.type))
  const values = form => [...new FormData(form)].filter(([name]) => name !== 'authenticity_token')
  const signature = form => JSON.stringify([form.action, values(form).sort(([a, av], [b, bv]) => a.localeCompare(b) || String(av).localeCompare(String(bv)))])
  const dirty = () => !!state?.form.isConnected && signature(state.form) !== state.baseline
  const guarded = () => !bypass && !state?.submitting && dirty()

  function unload(event) {
    if (!guarded()) return
    event.preventDefault()
    event.returnValue = ''
  }

  function watch() {
    // Keep clean settings pages eligible for the browser's back/forward cache.
    window.removeEventListener('beforeunload', unload)
    if (guarded()) window.addEventListener('beforeunload', unload)
  }

  function restore(form, entries) {
    const data = new Map()
    for (const [name, value] of entries) data.set(name, [...(data.get(name) || []), value])
    for (const field of fields(form)) {
      if (field.type === 'checkbox' || field.type === 'radio') field.checked = (data.get(field.name) || []).includes(field.value)
      else if (data.has(field.name)) field.value = data.get(field.name)[0]
    }
  }

  function connect() {
    currentIndex = history.state?.turbo?.restorationIndex
    const form = document.querySelector(formSelector)
    if (form === state?.form) return
    if (form && switchDraft?.key === form.dataset.emberSettings) {
      const previous = state.drafts.get(form.action)
      if (previous) restore(form, previous)
      for (const [name, value] of switchDraft.common) form.elements.namedItem(name).value = value
      state.form = form
    } else {
      state = form ? { form, baseline: signature(form), drafts: new Map(), submitting: false } : undefined
    }
    switchDraft = undefined
    continuation = undefined
    saving = false
    bypass = false
    watch()
  }

  function keepEditing() {
    if (saving) return
    continuation = undefined
    dialog()?.close()
  }

  function ask(resume) {
    if (saving) return
    continuation = resume
    const modal = dialog()
    modal.querySelector('[data-ember-save-error]').hidden = true
    if (!modal.open) modal.showModal()
  }

  function leave() {
    const resume = continuation
    continuation = undefined
    switchDraft = undefined
    bypass = true
    dialog()?.close()
    // A channel's permanent name/icon are only for switching its access form. They must
    // never migrate into a different channel, or resurrect discarded edits from a snapshot.
    state?.form.querySelectorAll('[data-turbo-permanent]').forEach(node => node.removeAttribute('data-turbo-permanent'))
    window.Turbo.cache.clear()
    watch()
    resume?.()
  }

  async function save() {
    if (saving || !state?.form) return
    const form = state.form
    if (!form.checkValidity()) {
      keepEditing()
      form.reportValidity()
      return
    }
    const modal = dialog()
    const button = modal.querySelector('[data-ember-save]')
    const error = modal.querySelector('[data-ember-save-error]')
    const controls = [...modal.querySelectorAll('button')]
    saving = true
    controls.forEach(control => { control.disabled = true })
    modal.setAttribute('aria-busy', 'true')
    button.textContent = 'Saving…'
    error.hidden = true
    try {
      // Use the same multipart endpoint and method override as the ordinary form, but keep
      // the current document intact until the save succeeds (including on a 422 or 500).
      const headers = { Accept: 'text/html', Prefer: 'return=minimal' }
      const csrf = document.querySelector('meta[name="csrf-token"]')?.content
      if (csrf) headers['X-CSRF-Token'] = csrf
      const response = await fetch(form.action, { method: form.method, body: new FormData(form), headers, credentials: 'same-origin' })
      if (response.status !== 204 || response.headers.get('Preference-Applied') !== 'return=minimal') throw new Error('save failed')
      state.baseline = signature(form)
      // The permanent sidebar must reload memberships even when the destination isn't a room.
      document.getElementById('user_sidebar')?.removeAttribute('data-turbo-permanent')
      leave()
    } catch {
      error.textContent = 'Your changes couldn’t be saved. Please try again, or keep editing.'
      error.hidden = false
    } finally {
      saving = false
      modal.removeAttribute('aria-busy')
      controls.forEach(control => { control.disabled = false })
      button.textContent = 'Save'
    }
  }

  function prepareSwitch(url) {
    state.drafts.set(state.form.action, values(state.form))
    switchDraft = { url, key: state.form.dataset.emberSettings, common: values(state.form).filter(([name]) => ['room[name]', 'room[icon]'].includes(name)) }
  }

  document.addEventListener('click', event => {
    const button = event.target.closest('[data-ember-keep-editing], [data-ember-discard], [data-ember-save]')
    if (button) {
      if (button.hasAttribute('data-ember-keep-editing')) keepEditing()
      else if (button.hasAttribute('data-ember-discard')) leave()
      else void save()
      return
    }
    const link = event.target.closest('a[href]')
    if (!link || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || link.hasAttribute('download') || (link.target && link.target !== '_self')) return
    // Opening the people picker leaves this draft in place; its eventual submit is guarded.
    if (link.matches('[data-ember-new-direct], [data-matchbox-new-direct]') && document.querySelector('[data-ember-dm-ready], [data-matchbox-dm-ready]')) return
    // Sidebar frame controls do not leave the settings document.
    const container = link.closest('turbo-frame')
    const frame = link.dataset.turboFrame || container?.getAttribute('target') || container?.id
    if (frame && frame !== '_top') return
    const url = new URL(link.href)
    if (!['http:', 'https:'].includes(url.protocol)) return
    if (url.origin === location.origin && url.pathname === location.pathname && url.search === location.search && url.hash) return
    if (state?.form.contains(link) && link.hasAttribute('data-ember-settings-switch')) {
      prepareSwitch(url.href)
      return
    }
    if (!guarded()) return
    event.preventDefault()
    event.stopImmediatePropagation()
    ask(() => {
      if (url.origin !== location.origin || link.closest('[data-turbo="false"]')) location.assign(url.href)
      else window.Turbo.visit(url.href, { action: link.dataset.turboAction || 'advance' })
    })
  }, true)

  // Cover programmatic Turbo visits as well as clicked links.
  document.addEventListener('turbo:before-visit', event => {
    if (switchDraft?.url === event.detail.url || !guarded()) return
    event.preventDefault()
    ask(() => window.Turbo.visit(event.detail.url))
  })
  document.addEventListener('submit', event => {
    if (!state || bypass) return
    if (event.target === state.form) {
      state.submitting = true
      watch()
    } else if (guarded() && !event.target.closest('turbo-frame')) {
      event.preventDefault()
      event.stopImmediatePropagation()
      ask(() => event.target.requestSubmit(event.submitter))
    }
  }, true)
  document.addEventListener('turbo:submit-end', event => {
    if (event.target !== state?.form) {
      if (bypass && !event.detail.success) { bypass = false; watch() }
      return
    }
    state.submitting = false
    if (event.detail.success) state.baseline = signature(state.form)
    watch()
  })
  for (const type of ['input', 'change', 'reset']) document.addEventListener(type, () => queueMicrotask(watch))
  document.addEventListener('keydown', event => {
    const modal = dialog()
    if (!modal?.open || event.key !== 'Tab') return
    const buttons = [...modal.querySelectorAll('button:not(:disabled)')]
    const index = buttons.indexOf(document.activeElement)
    if (event.shiftKey ? index <= 0 : index === buttons.length - 1) {
      event.preventDefault()
      buttons[event.shiftKey ? buttons.length - 1 : 0]?.focus()
    }
  })
  document.addEventListener('cancel', event => {
    if (event.target !== dialog()) return
    event.preventDefault()
    keepEditing()
  }, true)

  // Turbo's restore visits omit before-visit. Its vendored History stores an index in
  // history.state.turbo; return to the current entry before prompting, without creating
  // dummy entries or letting Turbo replace the edited document. Discard/Save then replay
  // the original traversal. Cross-document navigation uses the browser's unload prompt.
  window.addEventListener('popstate', event => {
    if (returningFromHistory) {
      event.stopImmediatePropagation()
      const delta = returningFromHistory
      returningFromHistory = undefined
      ask(() => history.go(delta))
      return
    }
    const nextIndex = event.state?.turbo?.restorationIndex
    if (!guarded() || !Number.isInteger(nextIndex) || !Number.isInteger(currentIndex) || nextIndex === currentIndex) return
    event.stopImmediatePropagation()
    const delta = nextIndex - currentIndex
    returningFromHistory = delta
    history.go(-delta)
  }, true)
  document.addEventListener('turbo:before-cache', () => {
    dialog()?.close()
    if (!switchDraft) state?.form.querySelectorAll('[data-turbo-permanent]').forEach(node => node.removeAttribute('data-turbo-permanent'))
  })
  document.addEventListener('turbo:render', connect)
  document.addEventListener('turbo:load', connect)
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', connect, { once: true })
  else connect()
  window.addEventListener('load', () => { currentIndex = history.state?.turbo?.restorationIndex }, { once: true })
})()

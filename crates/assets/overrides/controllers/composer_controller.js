// Based on reference/app/javascript/controllers/composer_controller.js.
// Bound attachments before upload and show transfer, processing, and failure states.
import { Controller } from "@hotwired/stimulus"
import FileUploader, { MAX_FILE_SIZE } from "models/file_uploader"
import { onNextEventLoopTick, nextFrame } from "helpers/timing_helpers"
import { escapeHTML } from "helpers/string_helpers"

export default class extends Controller {
  static classes = ["toolbar"]
  static targets = [ "clientid", "fields", "fileList", "text" ]
  static values = { roomId: Number }
  static outlets = [ "messages" ]

  #files = []
  #fileErrors = []

  connect() {
    this.#restoreDraft()

    if (!this.#usingTouchDevice) {
      onNextEventLoopTick(() => this.textTarget.focus())
    }
  }

  saveDraft() {
    if (this.textTarget.isBlank) {
      localStorage.removeItem(this.#draftKey)
    } else {
      localStorage.setItem(this.#draftKey, this.textTarget.value)
    }
  }

  submit(event) {
    event.preventDefault()

    if (!this.fieldsTarget.disabled) {
      this.#submitFiles()
      this.#submitMessage()
      this.collapseToolbar()
      this.textTarget.focus()
    }
  }

  submitEnd(event) {
    if (!event.detail.success) {
      this.messagesOutlet.failPendingMessage(this.clientidTarget.value)
    }
  }

  toggleToolbar() {
    this.element.classList.toggle(this.toolbarClass)
    this.textTarget.focus()
  }

  collapseToolbar() {
    this.element.classList.remove(this.toolbarClass)
  }

  replaceMessageContent(content) {
    this.textTarget.value = content
    this.textTarget.focus()
    this.textTarget.selection.placeCursorAtTheEnd()
  }

  submitByKeyboard(event) {
    if (event.key != "Enter" || this.textTarget.hasOpenPrompt) return

    const toolbarVisible = this.element.classList.contains(this.toolbarClass)
    const metaEnter = event.metaKey || event.ctrlKey
    const plainEnter = !event.shiftKey && !event.isComposing

    if (!this.#usingTouchDevice && (metaEnter || (plainEnter && !toolbarVisible))) {
      event.stopPropagation()
      this.submit(event)
    }
  }

  filePicked(event) {
    this.#addFiles(event.target.files)
    event.target.value = null
    this.#updateFileList()
  }

  fileUnpicked(event) {
    this.#files.splice(event.params.index, 1)
    this.#updateFileList()
  }

  pasteFiles(event) {
    if (event.clipboardData.files.length > 0) {
      event.preventDefault()
    }

    this.#addFiles(event.clipboardData.files)
    this.#updateFileList()
  }

  dropFiles({ detail: { files } }) {
    this.#addFiles(files)
    this.#updateFileList()
  }

  preventAttachment(event) {
    event.preventDefault()
  }

  online() {
    this.fieldsTarget.disabled = false
  }

  offline() {
    this.fieldsTarget.disabled = true
  }

  #restoreDraft() {
    const draft = localStorage.getItem(this.#draftKey)

    if (draft) {
      this.textTarget.value = draft
      this.textTarget.selection.placeCursorAtTheEnd()
    }
  }

  get #draftKey() {
    return `composer-draft-${this.roomIdValue}`
  }

  get #usingTouchDevice() {
    return 'ontouchstart' in window || navigator.maxTouchPoints > 0 || navigator.msMaxTouchPoints > 0;
  }

  async #submitMessage() {
    if (this.#validInput()) {
      const clientMessageId = this.#generateClientId()

      await this.messagesOutlet.insertPendingMessage(clientMessageId, this.textTarget)
      await nextFrame()

      this.clientidTarget.value = clientMessageId
      this.element.requestSubmit()
      this.#reset()
    }
  }

  #validInput() {
    return !this.textTarget.isBlank
  }

  async #submitFiles() {
    const files = this.#files

    this.#files = []
    this.#updateFileList()

    for (const file of files) {
      if (!this.element.isConnected || !this.hasMessagesOutlet) break
      const clientMessageId = this.#generateClientId()
      const uploader = new FileUploader(file, this.element.action, clientMessageId, this.#uploadProgress.bind(this))

      const body = this.#pendingUploadProgress(file)
      await this.messagesOutlet.insertPendingMessage(clientMessageId, body)

      try {
        const resp = await uploader.upload()
        Turbo.renderStreamMessage(resp)
      } catch (error) {
        // A successful Cable broadcast can arrive before a lost HTTP response. Keep that message.
        if (this.#pendingUploadExists(clientMessageId)) {
          this.messagesOutlet.updatePendingMessage(clientMessageId, this.#pendingUploadProgress(file, 0, error.message))
        }
      }
    }
  }

  #uploadProgress(percent, clientMessageId, file) {
    if (this.#pendingUploadExists(clientMessageId)) {
      this.messagesOutlet.updatePendingMessage(clientMessageId, this.#pendingUploadProgress(file, percent))
    }
  }

  #pendingUploadExists(clientMessageId) {
    return this.hasMessagesOutlet && document.getElementById(`pending_message_${clientMessageId}`)?.matches("[data-pending-message]")
  }

  #addFiles(files) {
    this.#fileErrors = []
    for (const file of files) {
      if (file.size > MAX_FILE_SIZE) this.#fileErrors.push(`${file.name} is too large. Choose a file that’s 250 MB or smaller.`)
      else this.#files.push(file)
    }
  }

  #generateClientId() {
    return Math.random().toString(36).slice(2)
  }

  #reset() {
    this.textTarget.value = ""
    localStorage.removeItem(this.#draftKey)
  }

  #updateFileList() {
    this.#files.sort((a, b) => a.name.localeCompare(b.name))

    const fileNodes = this.#files.map((file, index) => {
      const filename = file.name.split(".").slice(0, -1).join(".")
      const extension = file.name.split(".").pop()

      const node = document.createElement("button")
      node.setAttribute("type","button")
      node.setAttribute("style","gap: 0")
      node.dataset.action = "composer#fileUnpicked"
      node.dataset.composerIndexParam = index
      node.className = "btn btn--plain composer__file txt-normal position-relative unpad flex-column"
      node.innerHTML = file.type.match(/^image\/.*/) ? `<img role="presentation" class="flex-item-no-shrink composer__file-thumbnail" src="${URL.createObjectURL(file)}">` : `<span class="composer__file-thumbnail composer__file-thumbnail--common colorize--black"></span>`
      node.innerHTML += `<span class="pad-inline txt-small flex align-center max-width composer__file-caption"><span class="overflow-ellipsis">${escapeHTML(filename)}.</span><span class="flex-item-no-shrink">${escapeHTML(extension)}</span></span>`

      return node
    })

    this.fileListTarget.replaceChildren(...fileNodes)
    if (this.#fileErrors.length) {
      const error = document.createElement("p")
      error.className = "mb-upload-error"
      error.setAttribute("role", "alert")
      error.textContent = this.#fileErrors.join(" ")
      this.fileListTarget.append(error)
    }
  }

  #pendingUploadProgress(file, percent = 0, error = null) {
    const filename = escapeHTML(file.name)
    const size = file.size < 1_000_000 ? `${Math.max(1, Math.ceil(file.size / 1000))} KB` : `${(file.size / 1_000_000).toFixed(1)} MB`
    const status = error ? "Upload failed" : percent === 100 ? "Processing…" : `Uploading · ${percent}%`
    return `
      <div class="mb-upload${error ? " mb-upload--failed" : ""}" role="group" aria-label="Upload ${filename}">
        <div class="mb-upload-heading"><strong class="mb-upload-name">${filename}</strong><span class="mb-upload-size">${size}</span></div>
        ${error ? `<p class="mb-upload-error" role="alert">${escapeHTML(error)}</p>` : `<progress max="100" value="${percent}" aria-label="Upload progress for ${filename}"></progress>`}
        <span class="mb-upload-status" role="status">${status}</span>
      </div>
    `
  }
}

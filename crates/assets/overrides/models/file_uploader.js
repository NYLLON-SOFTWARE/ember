export const MAX_FILE_SIZE = 250_000_000

export default class FileUploader {
  constructor(file, url, clientMessageId, progressCallback) {
    this.file = file
    this.url = url
    this.clientMessageId = clientMessageId
    this.progressCallback = progressCallback
  }

  upload() {
    if (this.file.size > MAX_FILE_SIZE) return Promise.reject(new Error("Files must be 250 MB or smaller."))

    const formdata = new FormData()
    formdata.append("message[attachment]", this.file)
    formdata.append("message[client_message_id]", this.clientMessageId)

    const req = new XMLHttpRequest()
    req.open("POST", this.url)
    req.setRequestHeader("Accept", "text/vnd.turbo-stream.html")
    req.upload.addEventListener("progress", this.#uploadProgress.bind(this))

    const result = new Promise((resolve, reject) => {
      req.addEventListener("load", () => {
        if (req.status >= 200 && req.status < 300 && req.getResponseHeader("Content-Type")?.includes("text/vnd.turbo-stream.html")) {
          resolve(req.response)
        } else {
          reject(new Error(req.status === 413 ? "Files must be 250 MB or smaller." : "Upload failed. Please attach the file and try again."))
        }
      })
      req.addEventListener("error", () => reject(new Error("Connection lost. Please attach the file and try again.")))
      req.addEventListener("abort", () => reject(new Error("Upload canceled.")))
      req.addEventListener("timeout", () => reject(new Error("Upload timed out. Please attach the file and try again.")))
    })

    req.send(formdata)
    return result
  }

  #uploadProgress(event) {
    if (event.lengthComputable) {
      const percent = Math.min(100, Math.floor((event.loaded / event.total) * 100))
      this.progressCallback(percent, this.clientMessageId, this.file)
    }
  }
}

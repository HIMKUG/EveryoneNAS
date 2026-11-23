#include "flutter_window.h"

#include <flutter/standard_method_codec.h>
#include <shobjidl.h>
#include <windows.h>

#include <optional>
#include <string>

#include "flutter/generated_plugin_registrant.h"

namespace {

template <typename T>
class ComPtrGuard {
 public:
  ComPtrGuard() = default;
  ~ComPtrGuard() { Reset(); }
  ComPtrGuard(const ComPtrGuard&) = delete;
  ComPtrGuard& operator=(const ComPtrGuard&) = delete;

  T** Put() {
    Reset();
    return &ptr_;
  }
  T* Get() const { return ptr_; }
  T* operator->() const { return ptr_; }
  void Reset() {
    if (ptr_ != nullptr) {
      ptr_->Release();
      ptr_ = nullptr;
    }
  }

 private:
  T* ptr_ = nullptr;
};

std::wstring Utf8ToWide(const std::string& utf8) {
  if (utf8.empty()) {
    return L"";
  }
  const int needed = ::MultiByteToWideChar(
      CP_UTF8, 0, utf8.data(), static_cast<int>(utf8.size()), nullptr, 0);
  if (needed <= 0) {
    return L"";
  }
  std::wstring wide(static_cast<size_t>(needed), L'\0');
  ::MultiByteToWideChar(CP_UTF8, 0, utf8.data(), static_cast<int>(utf8.size()),
                        wide.data(), needed);
  return wide;
}

std::string WideToUtf8(const wchar_t* wide) {
  if (wide == nullptr || wide[0] == L'\0') {
    return "";
  }
  const int needed = ::WideCharToMultiByte(
      CP_UTF8, 0, wide, -1, nullptr, 0, nullptr, nullptr);
  if (needed <= 1) {
    return "";
  }
  std::string utf8(static_cast<size_t>(needed - 1), '\0');
  ::WideCharToMultiByte(
      CP_UTF8, 0, wide, -1, utf8.data(), needed, nullptr, nullptr);
  return utf8;
}

std::optional<std::string> ExtractTitleArg(
    const flutter::MethodCall<flutter::EncodableValue>& call) {
  const auto* args = std::get_if<flutter::EncodableMap>(call.arguments());
  if (args == nullptr) {
    return std::nullopt;
  }
  const auto it = args->find(flutter::EncodableValue("title"));
  if (it == args->end()) {
    return std::nullopt;
  }
  if (const auto* str = std::get_if<std::string>(&it->second)) {
    return *str;
  }
  return std::nullopt;
}

void ShowNativeItemDialog(
    HWND owner_hwnd,
    bool pick_folder,
    const std::wstring& title,
    std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>> result) {
  ComPtrGuard<IFileOpenDialog> dialog;
  HRESULT hr = ::CoCreateInstance(
      CLSID_FileOpenDialog, nullptr, CLSCTX_INPROC_SERVER,
      IID_PPV_ARGS(dialog.Put()));
  if (FAILED(hr) || dialog.Get() == nullptr) {
    result->Error("WIN32_DIALOG_INIT", "Failed to create IFileOpenDialog");
    return;
  }

  DWORD options = 0;
  hr = dialog->GetOptions(&options);
  if (SUCCEEDED(hr)) {
    DWORD flags = options | FOS_FORCEFILESYSTEM | FOS_NOCHANGEDIR;
    if (pick_folder) {
      flags |= FOS_PICKFOLDERS | FOS_PATHMUSTEXIST;
    } else {
      flags |= FOS_FILEMUSTEXIST;
    }
    dialog->SetOptions(flags);
  }

  if (!title.empty()) {
    dialog->SetTitle(title.c_str());
  }

  hr = dialog->Show(owner_hwnd);
  if (hr == HRESULT_FROM_WIN32(ERROR_CANCELLED)) {
    result->Success();
    return;
  }
  if (FAILED(hr)) {
    result->Error("WIN32_DIALOG_SHOW", "Native file dialog failed");
    return;
  }

  ComPtrGuard<IShellItem> item;
  hr = dialog->GetResult(item.Put());
  if (FAILED(hr) || item.Get() == nullptr) {
    result->Error("WIN32_DIALOG_RESULT", "Failed to read dialog selection");
    return;
  }

  PWSTR raw_path = nullptr;
  hr = item->GetDisplayName(SIGDN_FILESYSPATH, &raw_path);
  if (FAILED(hr) || raw_path == nullptr) {
    if (raw_path != nullptr) {
      ::CoTaskMemFree(raw_path);
    }
    result->Error("WIN32_DIALOG_PATH", "Failed to resolve selected filesystem path");
    return;
  }

  const std::string utf8_path = WideToUtf8(raw_path);
  ::CoTaskMemFree(raw_path);
  if (utf8_path.empty()) {
    result->Success();
    return;
  }
  result->Success(flutter::EncodableValue(utf8_path));
}

}  // namespace

FlutterWindow::FlutterWindow(const flutter::DartProject& project)
    : project_(project) {}

FlutterWindow::~FlutterWindow() {}

bool FlutterWindow::OnCreate() {
  if (!Win32Window::OnCreate()) {
    return false;
  }

  RECT frame = GetClientArea();

  // The size here must match the window dimensions to avoid unnecessary surface
  // creation / destruction in the startup path.
  flutter_controller_ = std::make_unique<flutter::FlutterViewController>(
      frame.right - frame.left, frame.bottom - frame.top, project_);
  // Ensure that basic setup of the controller was successful.
  if (!flutter_controller_->engine() || !flutter_controller_->view()) {
    return false;
  }
  RegisterPlugins(flutter_controller_->engine());

  keep_alive_channel_ =
      std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
          flutter_controller_->engine()->messenger(),
          "com.lanvault.app/keep_alive",
          &flutter::StandardMethodCodec::GetInstance());
  keep_alive_channel_->SetMethodCallHandler(
      [this](const flutter::MethodCall<flutter::EncodableValue>& call,
             std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>>
                 result) {
        const std::string& method = call.method_name();
        if (method == "pickDirectory") {
          const auto title_utf8 = ExtractTitleArg(call);
          const std::wstring title =
              title_utf8 ? Utf8ToWide(*title_utf8) : L"选择文件夹";
          ShowNativeItemDialog(GetHandle(), true, title, std::move(result));
          return;
        }
        if (method == "pickFile") {
          const auto title_utf8 = ExtractTitleArg(call);
          const std::wstring title =
              title_utf8 ? Utf8ToWide(*title_utf8) : L"选择文件";
          ShowNativeItemDialog(GetHandle(), false, title, std::move(result));
          return;
        }
        result->NotImplemented();
      });

  SetChildContent(flutter_controller_->view()->GetNativeWindow());

  flutter_controller_->engine()->SetNextFrameCallback([&]() {
    this->Show();
  });

  // Flutter can complete the first frame before the "show window" callback is
  // registered. The following call ensures a frame is pending to ensure the
  // window is shown. It is a no-op if the first frame hasn't completed yet.
  flutter_controller_->ForceRedraw();

  return true;
}

void FlutterWindow::OnDestroy() {
  if (keep_alive_channel_) {
    keep_alive_channel_->SetMethodCallHandler(nullptr);
    keep_alive_channel_ = nullptr;
  }
  if (flutter_controller_) {
    flutter_controller_ = nullptr;
  }

  Win32Window::OnDestroy();
}

LRESULT
FlutterWindow::MessageHandler(HWND hwnd, UINT const message,
                              WPARAM const wparam,
                              LPARAM const lparam) noexcept {
  // Give Flutter, including plugins, an opportunity to handle window messages.
  if (flutter_controller_) {
    std::optional<LRESULT> result =
        flutter_controller_->HandleTopLevelWindowProc(hwnd, message, wparam,
                                                      lparam);
    if (result) {
      return *result;
    }
  }

  switch (message) {
    case WM_FONTCHANGE:
      flutter_controller_->engine()->ReloadSystemFonts();
      break;
  }

  return Win32Window::MessageHandler(hwnd, message, wparam, lparam);
}

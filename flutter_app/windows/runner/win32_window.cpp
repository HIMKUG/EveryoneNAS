#include "win32_window.h"

#include <commctrl.h>
#include <dwmapi.h>
#include <flutter_windows.h>
#include <windowsx.h>

#include "resource.h"

namespace {

/// Window attribute that enables dark mode window decorations.
///
/// Redefined in case the developer's machine has a Windows SDK older than
/// version 10.0.22000.0.
/// See: https://docs.microsoft.com/windows/win32/api/dwmapi/ne-dwmapi-dwmwindowattribute
#ifndef DWMWA_USE_IMMERSIVE_DARK_MODE
#define DWMWA_USE_IMMERSIVE_DARK_MODE 20
#endif

/// Window corner preference (Windows 11); older DWM builds ignore it.
#ifndef DWMWA_WINDOW_CORNER_PREFERENCE
#define DWMWA_WINDOW_CORNER_PREFERENCE 33
#endif
#ifndef DWMWCP_ROUND
#define DWMWCP_ROUND 2
#endif

constexpr const wchar_t kWindowClassName[] = L"FLUTTER_RUNNER_WIN32_WINDOW";

/// Registry key for app theme preference.
///
/// A value of 0 indicates apps should use dark mode. A non-zero or missing
/// value indicates apps should use light mode.
constexpr const wchar_t kGetPreferredBrightnessRegKey[] =
  L"Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";
constexpr const wchar_t kGetPreferredBrightnessRegValue[] = L"AppsUseLightTheme";

// Caption band height used until the Flutter title bar reports its own metrics.
constexpr double kDefaultCaptionHeight = 44.0;

constexpr int kMinimumWindowWidth = 640;
constexpr int kMinimumWindowHeight = 480;

constexpr UINT_PTR kFlutterViewSubclassId = 1;

// The number of Win32Window objects that currently exist.
static int g_active_window_count = 0;

using EnableNonClientDpiScaling = BOOL __stdcall(HWND hwnd);

// Scale helper to convert logical scaler values to physical using passed in
// scale factor
int Scale(int source, double scale_factor) {
  return static_cast<int>(source * scale_factor);
}

// Dynamically loads the |EnableNonClientDpiScaling| from the User32 module.
// This API is only needed for PerMonitor V1 awareness mode.
void EnableFullDpiSupportIfAvailable(HWND hwnd) {
  HMODULE user32_module = LoadLibraryA("User32.dll");
  if (!user32_module) {
    return;
  }
  auto enable_non_client_dpi_scaling =
      reinterpret_cast<EnableNonClientDpiScaling*>(
          GetProcAddress(user32_module, "EnableNonClientDpiScaling"));
  if (enable_non_client_dpi_scaling != nullptr) {
    enable_non_client_dpi_scaling(hwnd);
  }
  FreeLibrary(user32_module);
}

// Loads the multi-resolution brand icon at the exact pixel sizes Windows asks for
// (small = title bar / taskbar, big = Alt-Tab, plus the two DPI-scaled variants),
// so the app corner icon is the same artwork as `assets/brand/logo_mark.png`.
void ApplyBrandIcons(HWND window) {
  const int sizes[] = {
      GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON),
      GetSystemMetrics(SM_CXICON),   GetSystemMetrics(SM_CYICON),
  };
  for (int size : sizes) {
    if (size <= 0) {
      continue;
    }
    auto icon = reinterpret_cast<HICON>(LoadImage(
        GetModuleHandle(nullptr), MAKEINTRESOURCE(IDI_APP_ICON),
        IMAGE_ICON, size, size, LR_DEFAULTCOLOR));
    if (icon == nullptr) {
      continue;
    }
    if (size <= GetSystemMetrics(SM_CXSMICON)) {
      SendMessage(window, WM_SETICON, ICON_SMALL, reinterpret_cast<LPARAM>(icon));
    } else {
      SendMessage(window, WM_SETICON, ICON_BIG, reinterpret_cast<LPARAM>(icon));
    }
  }
}

// Rounds the window corners on Windows 11 so the borderless frame still reads as
// a native window.
void ApplyFramelessChrome(HWND window) {
  const int corner_preference = DWMWCP_ROUND;
  DwmSetWindowAttribute(window, DWMWA_WINDOW_CORNER_PREFERENCE,
                        &corner_preference, sizeof(corner_preference));
}

// The hosted Flutter view is a child window that answers every WM_NCHITTEST with
// HTCLIENT, which would hide the app-drawn caption band from the top-level
// window. This subclass answers HTTRANSPARENT for the caption band so Windows
// walks up to the top-level window and runs its standard non-client handling
// there (drag, double-click maximize, Aero snap, edge resizing, system menu).
LRESULT CALLBACK FlutterViewHitTestProc(HWND window,
                                        UINT const message,
                                        WPARAM const wparam,
                                        LPARAM const lparam,
                                        UINT_PTR subclass_id,
                                        DWORD_PTR ref_data) {
  if (message == WM_NCDESTROY) {
    RemoveWindowSubclass(window, FlutterViewHitTestProc, subclass_id);
    return DefSubclassProc(window, message, wparam, lparam);
  }

  auto* that = reinterpret_cast<Win32Window*>(ref_data);
  if (message == WM_NCHITTEST && that != nullptr && that->GetHandle() != nullptr) {
    const POINT screen_point = {GET_X_LPARAM(lparam), GET_Y_LPARAM(lparam)};
    if (that->HitTestCustomFrame(that->GetHandle(), screen_point) != HTCLIENT) {
      return HTTRANSPARENT;
    }
  }
  return DefSubclassProc(window, message, wparam, lparam);
}

}  // namespace

// Manages the Win32Window's window class registration.
class WindowClassRegistrar {
 public:
  ~WindowClassRegistrar() = default;

  // Returns the singleton registrar instance.
  static WindowClassRegistrar* GetInstance() {
    if (!instance_) {
      instance_ = new WindowClassRegistrar();
    }
    return instance_;
  }

  // Returns the name of the window class, registering the class if it hasn't
  // previously been registered.
  const wchar_t* GetWindowClass();

  // Unregisters the window class. Should only be called if there are no
  // instances of the window.
  void UnregisterWindowClass();

 private:
  WindowClassRegistrar() = default;

  static WindowClassRegistrar* instance_;

  bool class_registered_ = false;
};

WindowClassRegistrar* WindowClassRegistrar::instance_ = nullptr;

const wchar_t* WindowClassRegistrar::GetWindowClass() {
  if (!class_registered_) {
    WNDCLASS window_class{};
    window_class.hCursor = LoadCursor(nullptr, IDC_ARROW);
    window_class.lpszClassName = kWindowClassName;
    window_class.style = CS_HREDRAW | CS_VREDRAW;
    window_class.cbClsExtra = 0;
    window_class.cbWndExtra = 0;
    window_class.hInstance = GetModuleHandle(nullptr);
    window_class.hIcon =
        LoadIcon(window_class.hInstance, MAKEINTRESOURCE(IDI_APP_ICON));
    window_class.hbrBackground = 0;
    window_class.lpszMenuName = nullptr;
    window_class.lpfnWndProc = Win32Window::WndProc;
    RegisterClass(&window_class);
    class_registered_ = true;
  }
  return kWindowClassName;
}

void WindowClassRegistrar::UnregisterWindowClass() {
  UnregisterClass(kWindowClassName, nullptr);
  class_registered_ = false;
}

Win32Window::Win32Window() {
  ++g_active_window_count;
}

Win32Window::~Win32Window() {
  --g_active_window_count;
  Destroy();
}

bool Win32Window::Create(const std::wstring& title,
                         const Point& origin,
                         const Size& size) {
  Destroy();

  const wchar_t* window_class =
      WindowClassRegistrar::GetInstance()->GetWindowClass();

  const POINT target_point = {static_cast<LONG>(origin.x),
                              static_cast<LONG>(origin.y)};
  HMONITOR monitor = MonitorFromPoint(target_point, MONITOR_DEFAULTTONEAREST);
  UINT dpi = FlutterDesktopGetDpiForMonitor(monitor);
  double scale_factor = dpi / 96.0;

  // Borderless, but the sizing border is kept so edge resizing, Aero snap and
  // the DWM drop shadow keep working. The frame itself is removed in
  // WM_NCCALCSIZE; every visible pixel is drawn by Flutter.
  const DWORD style = WS_POPUP | WS_THICKFRAME | WS_MINIMIZEBOX |
                      WS_MAXIMIZEBOX | WS_SYSMENU | WS_CLIPCHILDREN;

  HWND window = CreateWindowEx(
      WS_EX_APPWINDOW, window_class, title.c_str(), style,
      Scale(origin.x, scale_factor), Scale(origin.y, scale_factor),
      Scale(size.width, scale_factor), Scale(size.height, scale_factor),
      nullptr, nullptr, GetModuleHandle(nullptr), this);

  if (!window) {
    return false;
  }

  custom_caption_height_ = kDefaultCaptionHeight;

  // Flutter's generated runner only sets `hIcon` on the window class, which makes
  // Windows downscale one bitmap for the title bar. Load the brand icon at its native
  // per-surface sizes so the corner/taskbar/alt-tab icons stay identical to the logo
  // shown in Settings (which loads `assets/brand/logo_mark.png`).
  ApplyBrandIcons(window);
  ApplyFramelessChrome(window);

  UpdateTheme(window);

  return OnCreate();
}

bool Win32Window::Show() {
  return ShowWindow(window_handle_, SW_SHOWNORMAL);
}

void Win32Window::SetCustomFrame(double caption_height_logical,
                                 std::vector<FrameRect> excluded_logical) {
  custom_caption_height_ =
      caption_height_logical > 0.0 ? caption_height_logical : 0.0;
  custom_frame_excluded_.clear();
  for (const FrameRect& rect : excluded_logical) {
    if (rect.right > rect.left && rect.bottom > rect.top) {
      custom_frame_excluded_.push_back(rect);
    }
  }
  if (window_handle_ != nullptr) {
    SetWindowPos(window_handle_, nullptr, 0, 0, 0, 0,
                 SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE |
                     SWP_FRAMECHANGED);
  }
}

LRESULT Win32Window::HitTestCustomFrame(HWND window, POINT screen_point) {
  if (window == nullptr || custom_caption_height_ <= 0.0) {
    return HTCLIENT;
  }

  RECT window_rect{};
  if (!GetWindowRect(window, &window_rect)) {
    return HTCLIENT;
  }

  const UINT dpi = GetDpiForWindow(window);
  const double scale = dpi == 0 ? 1.0 : dpi / 96.0;
  const LONG physical_x = screen_point.x - window_rect.left;
  const LONG physical_y = screen_point.y - window_rect.top;
  const LONG physical_width = window_rect.right - window_rect.left;
  const LONG physical_height = window_rect.bottom - window_rect.top;

  // Resize grips along the outer edge, measured with the same system metrics
  // Windows uses for a sizing border. Skipped while maximized or minimized,
  // where a resize cannot start.
  if (!IsZoomed(window) && !IsIconic(window)) {
    const int grip_x = GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi) +
                       GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
    const int grip_y = GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi) +
                       GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
    const bool on_left = physical_x < grip_x;
    const bool on_right = physical_x >= physical_width - grip_x;
    const bool on_top = physical_y < grip_y;
    const bool on_bottom = physical_y >= physical_height - grip_y;
    if (on_top && on_left) {
      return HTTOPLEFT;
    }
    if (on_top && on_right) {
      return HTTOPRIGHT;
    }
    if (on_bottom && on_left) {
      return HTBOTTOMLEFT;
    }
    if (on_bottom && on_right) {
      return HTBOTTOMRIGHT;
    }
    if (on_left) {
      return HTLEFT;
    }
    if (on_right) {
      return HTRIGHT;
    }
    if (on_top) {
      return HTTOP;
    }
    if (on_bottom) {
      return HTBOTTOM;
    }
  }

  const double x = physical_x / scale;
  const double y = physical_y / scale;
  if (y >= 0.0 && y < custom_caption_height_) {
    for (const FrameRect& rect : custom_frame_excluded_) {
      if (x >= rect.left && x <= rect.right && y >= rect.top &&
          y <= rect.bottom) {
        return HTCLIENT;
      }
    }
    return HTCAPTION;
  }

  return HTCLIENT;
}

// static
LRESULT CALLBACK Win32Window::WndProc(HWND const window,
                                      UINT const message,
                                      WPARAM const wparam,
                                      LPARAM const lparam) noexcept {
  if (message == WM_NCCREATE) {
    auto window_struct = reinterpret_cast<CREATESTRUCT*>(lparam);
    SetWindowLongPtr(window, GWLP_USERDATA,
                     reinterpret_cast<LONG_PTR>(window_struct->lpCreateParams));

    auto that = static_cast<Win32Window*>(window_struct->lpCreateParams);
    EnableFullDpiSupportIfAvailable(window);
    that->window_handle_ = window;
  } else if (Win32Window* that = GetThisFromHandle(window)) {
    return that->MessageHandler(window, message, wparam, lparam);
  }

  return DefWindowProc(window, message, wparam, lparam);
}

LRESULT
Win32Window::MessageHandler(HWND hwnd,
                            UINT const message,
                            WPARAM const wparam,
                            LPARAM const lparam) noexcept {
  switch (message) {
    case WM_DESTROY:
      window_handle_ = nullptr;
      Destroy();
      if (quit_on_close_) {
        PostQuitMessage(0);
      }
      return 0;

    case WM_NCCALCSIZE:
      // The window is fully drawn by the app: dropping the non-client area makes
      // the client rect cover the whole window, caption band included.
      if (wparam == TRUE) {
        return 0;
      }
      break;

    case WM_NCHITTEST: {
      // Answer explicitly instead of falling through to DefWindowProc, whose
      // sizing-border inference no longer matches a zero-width non-client area.
      const POINT screen_point = {GET_X_LPARAM(lparam), GET_Y_LPARAM(lparam)};
      return HitTestCustomFrame(hwnd, screen_point);
    }

    case WM_GETMINMAXINFO: {
      auto* info = reinterpret_cast<MINMAXINFO*>(lparam);
      const UINT dpi = GetDpiForWindow(hwnd);
      const double scale = dpi == 0 ? 1.0 : dpi / 96.0;
      info->ptMinTrackSize.x = static_cast<LONG>(kMinimumWindowWidth * scale);
      info->ptMinTrackSize.y = static_cast<LONG>(kMinimumWindowHeight * scale);
      // A borderless popup would otherwise maximize over the taskbar; pin the
      // maximized rect to the monitor work area, since there is no native frame
      // for DefWindowProc to derive it from.
      MONITORINFO monitor_info{};
      monitor_info.cbSize = sizeof(monitor_info);
      if (GetMonitorInfo(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                         &monitor_info)) {
        info->ptMaxPosition.x =
            monitor_info.rcWork.left - monitor_info.rcMonitor.left;
        info->ptMaxPosition.y =
            monitor_info.rcWork.top - monitor_info.rcMonitor.top;
        info->ptMaxSize.x = monitor_info.rcWork.right - monitor_info.rcWork.left;
        info->ptMaxSize.y =
            monitor_info.rcWork.bottom - monitor_info.rcWork.top;
        info->ptMaxTrackSize.x =
            monitor_info.rcMonitor.right - monitor_info.rcMonitor.left;
        info->ptMaxTrackSize.y =
            monitor_info.rcMonitor.bottom - monitor_info.rcMonitor.top;
      }
      return 0;
    }

    case WM_SYSCOMMAND:
      // No menu bar exists, so swallow the Alt-key system menu activation
      // instead of letting Windows beep.
      if ((wparam & 0xFFF0) == SC_KEYMENU && lparam == 0) {
        return 0;
      }
      break;

    case WM_DPICHANGED: {
      auto newRectSize = reinterpret_cast<RECT*>(lparam);
      LONG newWidth = newRectSize->right - newRectSize->left;
      LONG newHeight = newRectSize->bottom - newRectSize->top;

      SetWindowPos(hwnd, nullptr, newRectSize->left, newRectSize->top, newWidth,
                   newHeight, SWP_NOZORDER | SWP_NOACTIVATE);

      return 0;
    }
    case WM_SIZE: {
      RECT rect = GetClientArea();
      if (child_content_ != nullptr) {
        // Size and position the child window.
        MoveWindow(child_content_, rect.left, rect.top, rect.right - rect.left,
                   rect.bottom - rect.top, TRUE);
      }
      return 0;
    }

    case WM_ACTIVATE:
      if (child_content_ != nullptr) {
        SetFocus(child_content_);
      }
      return 0;

    case WM_DWMCOLORIZATIONCOLORCHANGED:
      UpdateTheme(hwnd);
      return 0;
  }

  return DefWindowProc(window_handle_, message, wparam, lparam);
}

void Win32Window::Destroy() {
  OnDestroy();

  if (window_handle_) {
    DestroyWindow(window_handle_);
    window_handle_ = nullptr;
  }
  if (g_active_window_count == 0) {
    WindowClassRegistrar::GetInstance()->UnregisterWindowClass();
  }
}

Win32Window* Win32Window::GetThisFromHandle(HWND const window) noexcept {
  return reinterpret_cast<Win32Window*>(
      GetWindowLongPtr(window, GWLP_USERDATA));
}

void Win32Window::SetChildContent(HWND content) {
  child_content_ = content;
  SetParent(content, window_handle_);
  RECT frame = GetClientArea();

  MoveWindow(content, frame.left, frame.top, frame.right - frame.left,
             frame.bottom - frame.top, true);

  SetWindowSubclass(content, FlutterViewHitTestProc, kFlutterViewSubclassId,
                    reinterpret_cast<DWORD_PTR>(this));

  SetFocus(child_content_);
}

RECT Win32Window::GetClientArea() {
  RECT frame;
  GetClientRect(window_handle_, &frame);
  return frame;
}

HWND Win32Window::GetHandle() {
  return window_handle_;
}

void Win32Window::SetQuitOnClose(bool quit_on_close) {
  quit_on_close_ = quit_on_close;
}

bool Win32Window::OnCreate() {
  // No-op; provided for subclasses.
  return true;
}

void Win32Window::OnDestroy() {
  // No-op; provided for subclasses.
}

void Win32Window::UpdateTheme(HWND const window) {
  DWORD light_mode;
  DWORD light_mode_size = sizeof(light_mode);
  LSTATUS result = RegGetValue(HKEY_CURRENT_USER, kGetPreferredBrightnessRegKey,
                               kGetPreferredBrightnessRegValue,
                               RRF_RT_REG_DWORD, nullptr, &light_mode,
                               &light_mode_size);

  if (result == ERROR_SUCCESS) {
    BOOL enable_dark_mode = light_mode == 0;
    DwmSetWindowAttribute(window, DWMWA_USE_IMMERSIVE_DARK_MODE,
                          &enable_dark_mode, sizeof(enable_dark_mode));
  }
}

import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// Windows 原生窗口边框通道：驱动无边框窗口的最小化 / 最大化 / 关闭，
/// 并把 Flutter 自绘标题栏的命中区域同步给 Win32 侧。
///
/// 拖动、双击最大化、贴边分屏和窗口缩放全部由 Win32 非客户区处理，
/// 因此这里不拦截任何指针手势，只在标题栏按钮和窗口尺寸变化时通信。
class WindowChrome {
  WindowChrome._();

  static const MethodChannel _channel = MethodChannel('com.lanvault.app/window');

  static bool _handlerInstalled = false;
  static ValueChanged<bool>? _onMaximizedChanged;

  /// 当前平台是否为支持自绘标题栏的 Windows 桌面端。
  static bool get isSupported => !kIsWeb && Platform.isWindows;

  static void attach({ValueChanged<bool>? onMaximizedChanged}) {
    _onMaximizedChanged = onMaximizedChanged;
    if (!isSupported || _handlerInstalled) return;
    _channel.setMethodCallHandler((MethodCall call) async {
      if (call.method == 'maximizedChanged' && call.arguments is bool) {
        _onMaximizedChanged?.call(call.arguments as bool);
      }
      return null;
    });
    _handlerInstalled = true;
  }

  static void detach() {
    if (!_handlerInstalled) return;
    _handlerInstalled = false;
    _onMaximizedChanged = null;
    _channel.setMethodCallHandler(null);
  }

  static Future<void> minimize() => _invoke('minimize');

  static Future<void> toggleMaximize() => _invoke('toggleMaximize');

  static Future<void> close() => _invoke('close');

  static Future<bool> isMaximized() async {
    if (!isSupported) return false;
    try {
      return await _channel.invokeMethod<bool>('isMaximized') ?? false;
    } on PlatformException {
      return false;
    } on MissingPluginException {
      return false;
    }
  }

  /// 上报标题栏高度与其中的交互控件矩形（逻辑像素，相对窗口左上角）。
  /// 这些矩形保持为客户区，其余标题栏区域交给 Windows 作为拖动区。
  static Future<void> setCaptionMetrics({
    required double captionHeight,
    required List<Rect> exclusions,
  }) async {
    if (!isSupported) return;
    try {
      await _channel.invokeMethod<void>('setCaptionMetrics', <String, Object>{
        'captionHeight': captionHeight,
        'exclusions': <Map<String, double>>[
          for (final rect in exclusions)
            <String, double>{
              'left': rect.left,
              'top': rect.top,
              'right': rect.right,
              'bottom': rect.bottom,
            },
        ],
      });
    } on PlatformException {
      // 原生通道不可用时保持上一次的命中区域，不影响窗口继续工作。
    } on MissingPluginException {
      // 非 Windows 运行环境（测试 / 预览）直接忽略。
    }
  }

  static Future<void> _invoke(String method) async {
    if (!isSupported) return;
    try {
      await _channel.invokeMethod<void>(method);
    } on PlatformException {
      // 窗口可能已销毁，无需上层处理。
    } on MissingPluginException {
      // 非 Windows 运行环境直接忽略。
    }
  }
}

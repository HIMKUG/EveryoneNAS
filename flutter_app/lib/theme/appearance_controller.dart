import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import 'neu_theme.dart';
import 'skins.dart';

enum ThemeModePreference { system, light, dark }

class AppearanceController extends ChangeNotifier with WidgetsBindingObserver {
  AppearanceController._({
    required ThemeModePreference themeMode,
    required String skinId,
    required MotionSpeed motionSpeed,
    required Brightness platformBrightness,
    required bool observeSystem,
  })  : _themeMode = themeMode,
        _skinId = skinId,
        _motionSpeed = motionSpeed,
        _platformBrightness = platformBrightness,
        _observingSystem = observeSystem {
    if (observeSystem) WidgetsBinding.instance.addObserver(this);
  }

  factory AppearanceController.fromJson(
    Map<String, dynamic> json, {
    bool observeSystem = true,
  }) {
    final mode = ThemeModePreference.values.where(
      (value) => value.name == json['themeMode'],
    );
    final speed = MotionSpeed.values.where(
      (value) => value.name == json['motionSpeed'],
    );
    final skinId = json['skinId'];
    return AppearanceController._(
      themeMode: mode.isEmpty ? ThemeModePreference.system : mode.first,
      skinId: skinId is String && builtInSkins.any((skin) => skin.id == skinId)
          ? skinId
          : 'skin_default',
      motionSpeed: speed.isEmpty ? MotionSpeed.balanced : speed.first,
      platformBrightness:
          WidgetsBinding.instance.platformDispatcher.platformBrightness,
      observeSystem: observeSystem,
    );
  }

  static Future<AppearanceController> preload({
    Duration timeout = const Duration(milliseconds: 300),
    bool observeSystem = false,
  }) async {
    final stopwatch = Stopwatch()..start();
    while (true) {
      final loaded = await _readPreferences(
        stopwatch: stopwatch,
        timeout: timeout,
      );
      if (loaded != null) {
        return AppearanceController.fromJson(
          loaded,
          observeSystem: observeSystem,
        );
      }
      if (stopwatch.elapsed >= timeout) {
        return AppearanceController.memory(observeSystem: observeSystem);
      }
      final remaining = timeout - stopwatch.elapsed;
      await Future<void>.delayed(
        remaining < const Duration(milliseconds: 16)
            ? remaining
            : const Duration(milliseconds: 16),
      );
    }
  }

  factory AppearanceController.memory({
    ThemeModePreference themeMode = ThemeModePreference.system,
    String skinId = defaultSkinId,
    MotionSpeed motionSpeed = MotionSpeed.balanced,
    Brightness? platformBrightness,
    bool observeSystem = true,
  }) =>
      AppearanceController(
        themeMode: themeMode,
        skinId: skinId,
        motionSpeed: motionSpeed,
        platformBrightness: platformBrightness,
        observeSystem: observeSystem,
      );

  static Future<Map<String, dynamic>?> _readPreferences({
    required Stopwatch stopwatch,
    required Duration timeout,
  }) async {
    File? file;
    try {
      if (kIsWeb) return null;
      if (Platform.isWindows) {
        final localAppData = Platform.environment['LOCALAPPDATA'];
        if (localAppData == null || localAppData.isEmpty) return null;
        file = File(
          '$localAppData${Platform.pathSeparator}EveryoneNAS'
          '${Platform.pathSeparator}appearance_prefs.json',
        );
      } else if (Platform.isAndroid) {
        final filesDir =
            await _androidFilesChannel.invokeMethod<String>('getFilesDir');
        if (filesDir == null || filesDir.isEmpty) return null;
        file = File('$filesDir${Platform.pathSeparator}appearance_prefs.json');
      } else {
        return null;
      }
      final remaining = timeout - stopwatch.elapsed;
      if (remaining <= Duration.zero) return null;
      final result = await file.readAsString().timeout(remaining);
      final decoded = jsonDecode(result);
      return decoded is Map<String, dynamic> ? decoded : null;
    } on Object {
      return null;
    }
  }

  AppearanceController({
    ThemeModePreference themeMode = ThemeModePreference.system,
    String skinId = 'skin_default',
    MotionSpeed motionSpeed = MotionSpeed.balanced,
    Brightness? platformBrightness,
    File? preferencesFile,
    bool observeSystem = true,
  })  : _themeMode = themeMode,
        _skinId = skinId,
        _motionSpeed = motionSpeed,
        _platformBrightness = platformBrightness ??
            WidgetsBinding.instance.platformDispatcher.platformBrightness,
        _preferencesFile = preferencesFile {
    _observingSystem = observeSystem;
    if (observeSystem) {
      WidgetsBinding.instance.addObserver(this);
    }
  }

  static const MethodChannel _androidFilesChannel =
      MethodChannel('com.lanvault.app/keep_alive');

  ThemeModePreference _themeMode;
  String _skinId;
  MotionSpeed _motionSpeed;
  Brightness _platformBrightness;
  File? _preferencesFile;
  Future<void>? _pendingSave;
  bool _disposed = false;
  bool _observingSystem = false;

  ThemeModePreference get themeMode => _themeMode;
  String get skinId => _skinId;
  MotionSpeed get motionSpeed => _motionSpeed;
  Brightness get platformBrightness => _platformBrightness;

  void resumeSystemObservation() {
    if (_observingSystem || _disposed) return;
    _observingSystem = true;
    WidgetsBinding.instance.addObserver(this);
  }

  Brightness get effectiveBrightness => switch (_themeMode) {
        ThemeModePreference.light => Brightness.light,
        ThemeModePreference.dark => Brightness.dark,
        ThemeModePreference.system => _platformBrightness,
      };

  SkinDefinition get activeSkin {
    final skin = builtInSkins.where((candidate) => candidate.id == _skinId);
    return skin.isEmpty ? builtInSkins.first : skin.first;
  }

  SkinPalette get activePalette => activeSkin.paletteFor(effectiveBrightness);

  ThemeData get themeData => buildLanVaultTheme(
        palette: activePalette,
        brightness: effectiveBrightness,
        skinId: activeSkin.id,
        motionSpeed: _motionSpeed,
      );

  Future<void> load() async {
    final file = await _resolvePreferencesFile();
    _preferencesFile = file;
    if (file == null) return;

    try {
      if (!await file.exists()) return;
      final decoded = jsonDecode(await file.readAsString());
      if (decoded is! Map<String, dynamic>) return;
      final loaded = AppearanceController.fromJson(
        decoded,
        observeSystem: false,
      );
      final nextSkinId = loaded._skinId;
      final nextMode = loaded._themeMode;
      final nextSpeed = loaded._motionSpeed;
      _notifyIfChanged(nextMode, nextSkinId, nextSpeed);
    } on Object {
      return;
    }
  }

  void setThemeMode(ThemeModePreference mode) {
    _notifyIfChanged(mode, _skinId, _motionSpeed);
  }

  void toggleThemeMode() {
    setThemeMode(effectiveBrightness == Brightness.dark
        ? ThemeModePreference.light
        : ThemeModePreference.dark);
  }

  void toggleLightDark() => toggleThemeMode();

  void setSkinId(String skinId) {
    if (!builtInSkins.any((skin) => skin.id == skinId)) return;
    _notifyIfChanged(_themeMode, skinId, _motionSpeed);
  }

  void setSkin(SkinDefinition skin) => setSkinId(skin.id);

  void setMotionSpeed(MotionSpeed speed) {
    _notifyIfChanged(_themeMode, _skinId, speed);
  }

  Future<void> _notifyIfChanged(
    ThemeModePreference mode,
    String skinId,
    MotionSpeed speed,
  ) async {
    if (_themeMode == mode && _skinId == skinId && _motionSpeed == speed) {
      return;
    }
    _themeMode = mode;
    _skinId = skinId;
    _motionSpeed = speed;
    if (!_disposed) notifyListeners();
    await _scheduleSave();
  }

  Future<void> _scheduleSave() async {
    await _pendingSave;
    if (_disposed) return;
    final completer = Completer<void>();
    _pendingSave = completer.future;
    try {
      final file = _preferencesFile ?? await _resolvePreferencesFile();
      _preferencesFile = file;
      if (file != null) {
        final snapshot = jsonEncode(<String, Object>{
          'themeMode': _themeMode.name,
          'skinId': _skinId,
          'motionSpeed': _motionSpeed.name,
        });
        await file.parent.create(recursive: true);
        await file.writeAsString(snapshot, flush: true);
      }
      completer.complete();
    } on Object {
      completer.complete();
    }
  }

  Future<File?> _resolvePreferencesFile() async {
    if (kIsWeb || Platform.isWindows) {
      final localAppData = Platform.environment['LOCALAPPDATA'];
      if (localAppData == null || localAppData.isEmpty) return null;
      return File(
        '$localAppData${Platform.pathSeparator}EveryoneNAS'
        '${Platform.pathSeparator}appearance_prefs.json',
      );
    }
    if (Platform.isAndroid) {
      try {
        final filesDir =
            await _androidFilesChannel.invokeMethod<String>('getFilesDir');
        if (filesDir == null || filesDir.isEmpty) return null;
        return File('$filesDir${Platform.pathSeparator}appearance_prefs.json');
      } on MissingPluginException {
        return null;
      } on PlatformException {
        return null;
      }
    }
    return null;
  }

  @override
  void didChangePlatformBrightness() {
    final brightness =
        WidgetsBinding.instance.platformDispatcher.platformBrightness;
    if (_platformBrightness == brightness) return;
    _platformBrightness = brightness;
    if (!_disposed) notifyListeners();
  }

  @override
  void dispose() {
    if (!_disposed) {
      _disposed = true;
      WidgetsBinding.instance.removeObserver(this);
    }
    super.dispose();
  }
}

class AppearanceScope extends InheritedNotifier<AppearanceController> {
  const AppearanceScope({
    required super.notifier,
    required super.child,
    super.key,
  });

  static AppearanceController of(BuildContext context) {
    final scope = context.dependOnInheritedWidgetOfExactType<AppearanceScope>();
    assert(scope != null, 'AppearanceScope is missing above this context.');
    return scope!.notifier!;
  }

  static AppearanceController? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<AppearanceScope>()?.notifier;

  AppearanceController get controller => notifier!;
}

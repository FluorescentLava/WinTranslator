type
  TFinishedCheckSize = record
    Width, Height: Integer;
  end;

function OpenFinishedCheckTheme(Window: HWND; const Classes: String): THandle;
  external 'OpenThemeData@uxtheme.dll stdcall';
function GetFinishedCheckSize(Theme: THandle; DC: THandle; Part, State: Integer;
  Bounds: Integer; SizeType: Integer; var Size: TFinishedCheckSize): Integer;
  external 'GetThemePartSize@uxtheme.dll stdcall';
function CloseFinishedCheckTheme(Theme: THandle): Integer;
  external 'CloseThemeData@uxtheme.dll stdcall';

procedure ConfigureFinishedRunList;
var
  Theme: THandle;
  Size: TFinishedCheckSize;
begin
  Theme := OpenFinishedCheckTheme(WizardForm.RunList.Handle, 'Button');
  if Theme = 0 then Exit;
  try
    // Inno 6.7 reserves a fixed 13px bitmap column, but Windows draws the
    // themed checkbox at the current DPI. Offset also reserves space on the
    // left, so compensate for that width before the list clips the glyph.
    if GetFinishedCheckSize(Theme, 0, 3, 5, 0, 1, Size) = 0 then
      if Size.Width > 13 then
        WizardForm.RunList.Offset := Size.Width - 13 + ScaleX(2);
  finally
    CloseFinishedCheckTheme(Theme);
  end;
end;

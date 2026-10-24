rem npm run build
pnpm build
copy /Y public\assets\KikoSDK.min.js ..\sdk\javascript\.
del /S /Q ..\src\resources\assets\*
xcopy /S dist\* ..\src\resources\assets\.